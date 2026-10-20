//! Generates the Ruby and JRuby version tables in the Dev Center article "Heroku Ruby Support".

use jruby_executable::JRubyVersion;
use minijinja::{Environment, Value};
use ruby_executable::ruby_lang::RubyBranch;
use scraper::{ElementRef, Html, Selector};
use shared::devcenter::article::ArticleId;
use shared::{BaseImage, RubyDownloadVersion};
use std::fmt;
use std::str::FromStr;
use tokio::task::JoinSet;

pub mod guard;
pub mod jruby_table;
pub mod ruby_table;
pub mod update;

/// Dev Center id of <https://devcenter.heroku.com/articles/ruby-support-reference>.
pub const ARTICLE_ID: ArticleId = ArticleId(698);

/// Shown in a cell when nothing is available.
const UNAVAILABLE: &str = "–";

/// A `MAJOR.MINOR` release line, such as Ruby `3.4` or JRuby `10.0`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MajorMinor {
    pub major: u32,
    pub minor: u32,
}

impl fmt::Display for MajorMinor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.major, self.minor)
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("expected a `MAJOR.MINOR` version like `3.4` but got {0:?}")]
pub struct MajorMinorError(String);

impl FromStr for MajorMinor {
    type Err = MajorMinorError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        s.split_once('.')
            .and_then(|(major, minor)| {
                Some(MajorMinor {
                    major: major.parse().ok()?,
                    minor: minor.parse().ok()?,
                })
            })
            .ok_or_else(|| MajorMinorError(s.to_string()))
    }
}

impl From<&RubyDownloadVersion> for MajorMinor {
    fn from(version: &RubyDownloadVersion) -> Self {
        MajorMinor {
            major: version.major,
            minor: version.minor,
        }
    }
}

impl From<&JRubyVersion> for MajorMinor {
    fn from(version: &JRubyVersion) -> Self {
        MajorMinor {
            major: version.major(),
            minor: version.minor(),
        }
    }
}

impl From<&RubyBranch> for MajorMinor {
    fn from(branch: &RubyBranch) -> Self {
        MajorMinor {
            major: branch.major,
            minor: branch.minor,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum CheckError {
    #[error(transparent)]
    S3(#[from] shared::Error),

    #[error(transparent)]
    JRubyProperties(#[from] jruby_executable::Error),

    #[error("S3 check task failed: {0}")]
    Task(#[from] tokio::task::JoinError),
}

/// Every task's value, or the first error.
async fn join_all<T: 'static>(
    mut set: JoinSet<Result<T, CheckError>>,
) -> Result<Vec<T>, CheckError> {
    let mut values = Vec::new();
    while let Some(joined) = set.join_next().await {
        values.push(joined??);
    }
    Ok(values)
}

/// Renders a table template. Names ending in `.html` escape values for HTML.
fn render(name: &'static str, template: &'static str, context: Value) -> String {
    let mut environment = Environment::new();
    environment
        .add_template(name, template)
        .and_then(|()| environment.get_template(name)?.render(context))
        .expect("table templates render")
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ReadError {
    #[error("expected the table columns to start with {expected:?} but found {found:?}")]
    Columns {
        expected: &'static [&'static str],
        found: Vec<String>,
    },

    #[error("expected {expected} cells in each row but found {found:?}")]
    CellCount { expected: usize, found: Vec<String> },

    #[error("expected {expected} but found {found:?}")]
    Cell {
        expected: &'static str,
        found: String,
    },
}

/// The text of a table whose columns are `leading` followed by one column per base image.
struct TableText {
    base_images: Vec<BaseImage>,
    rows: Vec<Vec<String>>,
}

impl TableText {
    fn read(html: &str, leading: &'static [&'static str]) -> Result<TableText, ReadError> {
        let fragment = Html::parse_fragment(html);
        let texts = |parent: ElementRef, selector: &str| {
            let selector = Selector::parse(selector).expect("valid CSS selector");
            parent
                .select(&selector)
                .map(|element| element.text().collect::<String>().trim().to_string())
                .collect::<Vec<_>>()
        };

        let headers = texts(fragment.root_element(), "thead th");
        let base_images = match headers.split_at_checked(leading.len()) {
            Some((labels, base_images)) if labels == leading => base_images
                .iter()
                .map(|name| {
                    BaseImage::new(name).map_err(|_| ReadError::Cell {
                        expected: "a base image",
                        found: name.clone(),
                    })
                })
                .collect::<Result<Vec<_>, _>>()?,
            _ => {
                return Err(ReadError::Columns {
                    expected: leading,
                    found: headers,
                });
            }
        };

        let row_selector = Selector::parse("tbody tr").expect("valid CSS selector");
        let rows = fragment
            .select(&row_selector)
            .map(|row| {
                let cells = texts(row, "td");
                if cells.len() == headers.len() {
                    Ok(cells)
                } else {
                    Err(ReadError::CellCount {
                        expected: headers.len(),
                        found: cells,
                    })
                }
            })
            .collect::<Result<_, _>>()?;
        Ok(TableText { base_images, rows })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn major_minor_parses() {
        assert_eq!(
            "3.10".parse::<MajorMinor>(),
            Ok(MajorMinor {
                major: 3,
                minor: 10
            })
        );
        for invalid in ["3", "3.x", "3.4.1", ""] {
            assert!(invalid.parse::<MajorMinor>().is_err(), "{invalid:?}");
        }
    }

    #[test]
    fn major_minor_orders_numerically() {
        let older: MajorMinor = "3.9".parse().unwrap();
        let newer: MajorMinor = "3.10".parse().unwrap();
        assert!(older < newer);
    }

    #[test]
    fn reads_table_text_by_base_image_column() {
        let table = TableText::read(
            "<table><thead><tr><th>Label</th><th>heroku-24</th></tr></thead>\
             <tbody><tr><td> a </td><td><b>b</b></td></tr></tbody></table>",
            &["Label"],
        )
        .unwrap();

        assert_eq!(
            table.base_images,
            vec![BaseImage::new("heroku-24").unwrap()]
        );
        assert_eq!(table.rows, vec![vec!["a".to_string(), "b".to_string()]]);
    }

    #[test]
    fn rejects_unexpected_columns() {
        assert_eq!(
            TableText::read("<p>edited</p>", &["Label"]).err(),
            Some(ReadError::Columns {
                expected: &["Label"],
                found: vec![]
            })
        );
        assert_eq!(
            TableText::read(
                "<table><thead><tr><th>Label</th><th>cedar-14</th></tr></thead></table>",
                &["Label"]
            )
            .err(),
            Some(ReadError::Cell {
                expected: "a base image",
                found: "cedar-14".to_string()
            })
        );
    }

    #[test]
    fn rejects_a_row_that_does_not_match_the_columns() {
        assert_eq!(
            TableText::read(
                "<table><thead><tr><th>Label</th><th>heroku-24</th></tr></thead>\
                 <tbody><tr><td>a</td></tr></tbody></table>",
                &["Label"]
            )
            .err(),
            Some(ReadError::CellCount {
                expected: 2,
                found: vec!["a".to_string()]
            })
        );
    }
}
