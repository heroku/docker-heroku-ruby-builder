//! Refuses to publish a table that shows less than the article already does.
//!
//! Builds are never deleted from S3, so a cell that moves to an older version or to unavailable points at a bad read,
//! such as an S3 outage answering `403`, rather than a real change.

use crate::{Series, SeriesError};
use std::fmt;

/// A table row reduced to what the guard compares: its series and the version available on each stack.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuardRow {
    pub series: Series,
    cells: Vec<(String, Option<DottedVersion>)>,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum RowError {
    #[error("table row has no cells")]
    Empty,

    #[error(transparent)]
    Series(#[from] SeriesError),

    #[error("expected a dotted version like `3.4.1` but got {0:?}")]
    Version(String),

    #[error("row for {series} has {cells} stack cells but the table has {stacks} stack columns")]
    CellCount {
        series: Series,
        cells: usize,
        stacks: usize,
    },
}

impl GuardRow {
    /// `values` holds the version available on each of `stacks`, in the same order.
    pub fn new<'a>(
        series: Series,
        stacks: &[String],
        values: impl ExactSizeIterator<Item = Option<&'a str>>,
    ) -> Result<GuardRow, RowError> {
        if values.len() != stacks.len() {
            return Err(RowError::CellCount {
                series,
                cells: values.len(),
                stacks: stacks.len(),
            });
        }
        let cells = stacks
            .iter()
            .cloned()
            .zip(values)
            .map(|(stack, value)| Ok((stack, value.map(DottedVersion::parse).transpose()?)))
            .collect::<Result<_, RowError>>()?;
        Ok(GuardRow { series, cells })
    }
}

/// A version of numeric segments, ordered segment by segment, so `3.4.10 > 3.4.9` and `10.0.0.0 > 9.4.15.0`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct DottedVersion {
    text: String,
    segments: Vec<u32>,
}

impl DottedVersion {
    fn parse(text: &str) -> Result<DottedVersion, RowError> {
        text.split('.')
            .map(str::parse)
            .collect::<Result<Vec<u32>, _>>()
            .map(|segments| DottedVersion {
                text: text.to_string(),
                segments,
            })
            .map_err(|_| RowError::Version(text.to_string()))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Regression {
    RowRemoved {
        series: Series,
    },
    CellRemoved {
        series: Series,
        stack: String,
        was: String,
    },
    CellDowngraded {
        series: Series,
        stack: String,
        was: String,
        now: String,
    },
}

impl fmt::Display for Regression {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Regression::RowRemoved { series } => write!(f, "{series}: row removed"),
            Regression::CellRemoved { series, stack, was } => {
                write!(f, "{series} on {stack}: {was} became unavailable")
            }
            Regression::CellDowngraded {
                series,
                stack,
                was,
                now,
            } => write!(f, "{series} on {stack}: {was} became {now}"),
        }
    }
}

/// Every way `new` shows less than `old`.
///
/// Rows below `floor` may disappear. A stack column missing from `new` is ignored, since dropping a stack is a code
/// change rather than a bad read.
pub fn regressions(old: &[GuardRow], new: &[GuardRow], floor: Series) -> Vec<Regression> {
    let mut found = Vec::new();
    for old_row in old.iter().filter(|row| row.series >= floor) {
        let series = old_row.series;
        let Some(new_row) = new.iter().find(|row| row.series == series) else {
            found.push(Regression::RowRemoved { series });
            continue;
        };
        for (stack, was) in &old_row.cells {
            let Some(was) = was else { continue };
            let Some((_, now)) = new_row.cells.iter().find(|(name, _)| name == stack) else {
                continue;
            };
            match now {
                None => found.push(Regression::CellRemoved {
                    series,
                    stack: stack.clone(),
                    was: was.text.clone(),
                }),
                Some(now) if now.segments < was.segments => {
                    found.push(Regression::CellDowngraded {
                        series,
                        stack: stack.clone(),
                        was: was.text.clone(),
                        now: now.text.clone(),
                    })
                }
                Some(_) => {}
            }
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn series(s: &str) -> Series {
        s.parse().unwrap()
    }

    fn row(name: &str, values: &[Option<&str>]) -> GuardRow {
        let stacks = ["heroku-22", "heroku-24"].map(String::from);
        GuardRow::new(
            series(name),
            &stacks[..values.len()],
            values.iter().copied(),
        )
        .unwrap()
    }

    #[test]
    fn newer_and_newly_available_cells_pass() {
        let old = [row("3.4", &[Some("3.4.9"), None])];
        let new = [
            row("3.4", &[Some("3.4.10"), Some("3.4.10")]),
            row("4.0", &[Some("4.0.0"), None]),
        ];
        assert_eq!(regressions(&old, &new, series("3.1")), vec![]);
    }

    #[test]
    fn an_older_cell_is_a_regression() {
        let old = [row("3.4", &[Some("3.4.10"), Some("3.4.10")])];
        let new = [row("3.4", &[Some("3.4.9"), Some("3.4.10")])];
        assert_eq!(
            regressions(&old, &new, series("3.1")),
            vec![Regression::CellDowngraded {
                series: series("3.4"),
                stack: "heroku-22".to_string(),
                was: "3.4.10".to_string(),
                now: "3.4.9".to_string(),
            }]
        );
    }

    #[test]
    fn a_cell_becoming_unavailable_is_a_regression() {
        let old = [row("3.4", &[Some("3.4.1"), Some("3.4.1")])];
        let new = [row("3.4", &[Some("3.4.1"), None])];
        assert_eq!(
            regressions(&old, &new, series("3.1")),
            vec![Regression::CellRemoved {
                series: series("3.4"),
                stack: "heroku-24".to_string(),
                was: "3.4.1".to_string(),
            }]
        );
    }

    #[test]
    fn a_dropped_row_at_or_above_the_floor_is_a_regression() {
        let old = [row("3.4", &[Some("3.4.1")]), row("3.1", &[Some("3.1.7")])];
        let new = [row("3.4", &[Some("3.4.1")])];
        assert_eq!(
            regressions(&old, &new, series("3.1")),
            vec![Regression::RowRemoved {
                series: series("3.1")
            }]
        );
    }

    #[test]
    fn a_row_dropped_by_a_raised_floor_passes() {
        let old = [row("3.4", &[Some("3.4.1")]), row("3.1", &[Some("3.1.7")])];
        let new = [row("3.4", &[Some("3.4.1")])];
        assert_eq!(regressions(&old, &new, series("3.2")), vec![]);
    }

    #[test]
    fn a_removed_stack_column_passes() {
        let old = [row("3.4", &[Some("3.4.1"), Some("3.4.1")])];
        let new = [row("3.4", &[Some("3.4.1")])];
        assert_eq!(regressions(&old, &new, series("3.1")), vec![]);
    }

    #[test]
    fn versions_compare_by_numeric_segment() {
        let old = [row("10.0", &[Some("9.4.15.0")])];
        let new = [row("10.0", &[Some("10.0.0.0")])];
        assert_eq!(regressions(&old, &new, series("9.4")), vec![]);
    }

    #[test]
    fn rejects_rows_that_do_not_match_the_columns() {
        let stacks = ["heroku-22".to_string()];
        assert_eq!(
            GuardRow::new(series("3.4"), &stacks, [Some("3.4.1"), None].into_iter()),
            Err(RowError::CellCount {
                series: series("3.4"),
                cells: 2,
                stacks: 1
            })
        );
        assert_eq!(
            GuardRow::new(series("3.4"), &stacks, [Some("soon")].into_iter()),
            Err(RowError::Version("soon".to_string()))
        );
    }
}
