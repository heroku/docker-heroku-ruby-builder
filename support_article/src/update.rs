//! Splices freshly generated tables into the article, refusing any that regress.

use crate::Series;
use crate::guard::{self, GuardRow, Regression, RowError};
use crate::html_table::{self, Table};
use crate::markers::{self, MarkerError};

/// A table to write into the article region `name`.
pub struct GeneratedTable {
    pub name: &'static str,
    pub table: Table,
    /// Rows below this series may leave the table.
    pub floor: Series,
    pub guard_rows: fn(&Table) -> Result<Vec<GuardRow>, RowError>,
}

/// A region whose content changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    pub name: &'static str,
    pub before: String,
    pub after: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Update {
    pub content: String,
    pub changes: Vec<Change>,
}

#[derive(Debug, thiserror::Error)]
pub enum UpdateError {
    #[error(transparent)]
    Marker(#[from] MarkerError),

    #[error("cannot read the `{name}` table currently in the article: {source}")]
    CurrentTable {
        name: &'static str,
        source: html_table::ParseError,
    },

    #[error("cannot compare the `{name}` table: {source}")]
    Rows {
        name: &'static str,
        source: RowError,
    },

    #[error(
        "refusing to publish `{name}` because it shows less than the article does now. A bad read from S3 is the likely cause:\n{list}",
        list = regressions.iter().map(|regression| format!("- {regression}")).collect::<Vec<_>>().join("\n")
    )]
    Regressed {
        name: &'static str,
        regressions: Vec<Regression>,
    },
}

/// Replaces each table's region in `content`.
///
/// Fails when a region's markers are malformed, when its current table cannot be read, or when the new table shows
/// less than the current one. An empty region has nothing to compare against.
pub fn update(content: &str, tables: &[GeneratedTable]) -> Result<Update, UpdateError> {
    let mut content = content.to_string();
    let mut changes = Vec::new();
    for generated in tables {
        let name = generated.name;
        let before = markers::generated_region(&content, name)?.to_string();
        let after = generated.table.render();
        let rows = |table: &Table| {
            (generated.guard_rows)(table).map_err(|source| UpdateError::Rows { name, source })
        };

        if !before.is_empty() {
            let current = Table::parse(&before)
                .map_err(|source| UpdateError::CurrentTable { name, source })?;
            let regressions =
                guard::regressions(&rows(&current)?, &rows(&generated.table)?, generated.floor);
            if !regressions.is_empty() {
                return Err(UpdateError::Regressed { name, regressions });
            }
        }

        content = markers::replace_generated(&content, name, &after)?;
        if before.replace("\r\n", "\n") != after {
            changes.push(Change {
                name,
                before,
                after,
            });
        }
    }
    Ok(Update { content, changes })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::html_table::Row;
    use crate::ruby_table;
    use pretty_assertions::assert_eq;

    fn ruby(rows: &[&[&str]]) -> GeneratedTable {
        GeneratedTable {
            name: ruby_table::NAME,
            table: Table {
                headers: vec!["Ruby series".to_string(), "heroku-24".to_string()],
                rows: rows
                    .iter()
                    .map(|cells| Row {
                        style: None,
                        cells: cells.iter().map(|cell| cell.to_string()).collect(),
                    })
                    .collect(),
            },
            floor: "3.1".parse().unwrap(),
            guard_rows: ruby_table::guard_rows,
        }
    }

    fn article(region: &str) -> String {
        format!(
            "# Prose\r\n\r\n<!-- BEGIN GENERATED ruby_versions_table -->\r\n{region}\r\n<!-- END GENERATED ruby_versions_table -->\r\n\r\nMore prose\r\n"
        )
    }

    #[test]
    fn writes_a_newer_table_and_reports_the_change() {
        let current = ruby(&[&["3.4", "3.4.1"]])
            .table
            .render()
            .replace('\n', "\r\n");
        let newer = ruby(&[&["3.4", "3.4.2"]]);
        let expected = article(&newer.table.render().replace('\n', "\r\n"));

        let update = update(&article(&current), &[newer]).unwrap();

        assert_eq!(update.content, expected);
        assert_eq!(update.changes.len(), 1);
        assert_eq!(update.changes[0].before, current);
    }

    #[test]
    fn an_identical_table_changes_nothing() {
        let current = ruby(&[&["3.4", "3.4.1"]])
            .table
            .render()
            .replace('\n', "\r\n");
        let content = article(&current);

        let update = update(&content, &[ruby(&[&["3.4", "3.4.1"]])]).unwrap();

        assert_eq!(update.content, content);
        assert_eq!(update.changes, vec![]);
    }

    #[test]
    fn refuses_a_regressed_table() {
        let current = ruby(&[&["3.4", "3.4.2"]]).table.render();

        let error = update(&article(&current), &[ruby(&[&["3.4", "–"]])]).unwrap_err();

        assert!(
            matches!(&error, UpdateError::Regressed { regressions, .. } if regressions.len() == 1),
            "got {error}"
        );
        assert!(
            error
                .to_string()
                .contains("- 3.4 on heroku-24: 3.4.2 became unavailable"),
            "{error}"
        );
    }

    #[test]
    fn fills_an_empty_region_without_comparing() {
        let update = update(&article(""), &[ruby(&[&["3.4", "3.4.1"]])]).unwrap();

        assert_eq!(update.changes.len(), 1);
        assert!(update.content.contains("<td>3.4.1</td>"));
    }

    #[test]
    fn refuses_a_hand_edited_region() {
        let error = update(&article("<p>edited</p>"), &[ruby(&[&["3.4", "3.4.1"]])]).unwrap_err();

        assert!(
            matches!(error, UpdateError::CurrentTable { .. }),
            "got {error}"
        );
    }

    #[test]
    fn refuses_missing_markers() {
        let error = update("no markers here", &[ruby(&[&["3.4", "3.4.1"]])]).unwrap_err();

        assert!(matches!(error, UpdateError::Marker(_)), "got {error}");
    }
}
