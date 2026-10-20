//! Splices generated tables into the article, refusing any that regress.
//!
//! Each table owns the region between its marker comments, which Dev Center hides on the rendered page:
//!
//! ```text
//! <!-- BEGIN GENERATED ruby_versions_table -->
//! <table>...</table>
//! <!-- END GENERATED ruby_versions_table -->
//! ```

use crate::guard::{self, Regression, RowVersions};
use crate::{MajorMinor, ReadError};
use shared::BaseImage;
use std::fmt;

/// A table this tool generates into a region of the article.
pub trait VersionTable: Sized {
    /// Names the region's markers.
    const NAME: &'static str;

    type Version: Ord + Clone + fmt::Debug + fmt::Display;

    /// HTML with `\n` line endings.
    fn render(&self) -> String;

    /// Reads HTML produced by [`VersionTable::render`].
    fn read(html: &str) -> Result<Self, ReadError>;

    fn base_images(&self) -> &[BaseImage];

    fn row_versions(&self) -> Vec<RowVersions<Self::Version>>;
}

/// A region whose content changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    pub name: &'static str,
    pub before: String,
    pub after: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Replaced {
    pub content: String,
    pub change: Option<Change>,
}

#[derive(Debug, thiserror::Error)]
pub enum UpdateError<V: fmt::Debug + fmt::Display> {
    #[error(
        "expected the article to contain `<!-- BEGIN GENERATED {name} -->` and then `<!-- END GENERATED {name} -->`, once each"
    )]
    Markers { name: &'static str },

    #[error("cannot read the `{name}` table currently in the article: {source}")]
    CurrentTable {
        name: &'static str,
        source: ReadError,
    },

    #[error(
        "refusing to publish `{name}` because it shows less than the article does now. A bad read from S3 is the likely cause:\n{list}",
        list = regressions.iter().map(|regression| format!("- {regression}")).collect::<Vec<_>>().join("\n")
    )]
    Regressed {
        name: &'static str,
        regressions: Vec<Regression<V>>,
    },
}

/// Replaces the `T::NAME` region of `content` with `table`, keeping the article's line endings.
///
/// Fails when the region's markers are missing or repeated, when the table currently there cannot be read, or when
/// `table` shows less than it. Rows below `floor` may leave the table. An empty region has nothing to compare against.
pub fn replace_table<T: VersionTable>(
    content: &str,
    table: &T,
    floor: MajorMinor,
) -> Result<Replaced, UpdateError<T::Version>> {
    let begin = format!("<!-- BEGIN GENERATED {} -->", T::NAME);
    let end = format!("<!-- END GENERATED {} -->", T::NAME);
    let (head, region, tail) = (content.matches(&begin).count() == 1
        && content.matches(&end).count() == 1)
        .then(|| {
            let (head, rest) = content.split_once(&begin)?;
            let (region, tail) = rest.split_once(&end)?;
            Some((head, region.trim(), tail))
        })
        .flatten()
        .ok_or(UpdateError::Markers { name: T::NAME })?;

    if !region.is_empty() {
        let current = T::read(region).map_err(|source| UpdateError::CurrentTable {
            name: T::NAME,
            source,
        })?;
        let regressions = guard::regressions(
            &current.row_versions(),
            &table.row_versions(),
            table.base_images(),
            floor,
        );
        if !regressions.is_empty() {
            return Err(UpdateError::Regressed {
                name: T::NAME,
                regressions,
            });
        }
    }

    let after = table.render();
    let newline = if content.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    let generated = after.replace('\n', newline);
    Ok(Replaced {
        content: format!("{head}{begin}{newline}{generated}{newline}{end}{tail}"),
        change: (region.replace("\r\n", "\n") != after).then(|| Change {
            name: T::NAME,
            before: region.to_string(),
            after,
        }),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ruby_table::{Lifecycle, RubyRow, RubyTable};
    use pretty_assertions::assert_eq;
    use shared::RubyDownloadVersion;

    /// A one-column table with a row per `(line, version)`.
    fn table(rows: &[(&str, Option<&str>)]) -> RubyTable {
        let heroku_24 = BaseImage::new("heroku-24").unwrap();
        RubyTable {
            base_images: vec![heroku_24.clone()],
            rows: rows
                .iter()
                .map(|(major_minor, version)| RubyRow {
                    major_minor: major_minor.parse().unwrap(),
                    lifecycle: Lifecycle::Maintained,
                    versions: version
                        .map(|v| (heroku_24.clone(), RubyDownloadVersion::new(v).unwrap()))
                        .into_iter()
                        .collect(),
                })
                .collect(),
        }
    }

    fn floor() -> MajorMinor {
        "8.0".parse().unwrap()
    }

    fn article(region: &str) -> String {
        format!(
            "# Prose\r\n\r\n<!-- BEGIN GENERATED ruby_versions_table -->\r\n{region}\r\n<!-- END GENERATED ruby_versions_table -->\r\n\r\nMore prose\r\n"
        )
    }

    fn crlf(text: &str) -> String {
        text.replace('\n', "\r\n")
    }

    #[test]
    fn writes_a_newer_table_and_reports_the_change() {
        let current = crlf(&table(&[("8.1", Some("8.1.1"))]).render());
        let newer = table(&[("8.1", Some("8.1.2"))]);

        let replaced = replace_table(&article(&current), &newer, floor()).unwrap();

        assert_eq!(replaced.content, article(&crlf(&newer.render())));
        assert_eq!(
            replaced.change,
            Some(Change {
                name: "ruby_versions_table",
                before: current,
                after: newer.render(),
            })
        );
    }

    #[test]
    fn an_identical_table_changes_nothing() {
        let current = table(&[("8.1", Some("8.1.1"))]);
        let content = article(&crlf(&current.render()));

        let replaced = replace_table(&content, &current, floor()).unwrap();

        assert_eq!(replaced.content, content);
        assert_eq!(replaced.change, None);
    }

    #[test]
    fn keeps_lf_articles_lf() {
        let content = article("").replace("\r\n", "\n");
        let generated = table(&[("8.1", Some("8.1.1"))]);

        let replaced = replace_table(&content, &generated, floor()).unwrap();

        assert!(!replaced.content.contains('\r'), "{:?}", replaced.content);
        assert!(replaced.content.contains(&generated.render()));
    }

    #[test]
    fn refuses_a_regressed_table() {
        let current = table(&[("8.1", Some("8.1.2"))]).render();

        let error =
            replace_table(&article(&current), &table(&[("8.1", None)]), floor()).unwrap_err();

        assert!(
            error
                .to_string()
                .contains("- 8.1 on heroku-24: 8.1.2 became unavailable"),
            "{error}"
        );
    }

    #[test]
    fn fills_an_empty_region_without_comparing() {
        let replaced =
            replace_table(&article(""), &table(&[("8.1", Some("8.1.1"))]), floor()).unwrap();

        assert!(replaced.change.is_some());
        assert!(replaced.content.contains("<td>8.1.1</td>"));
    }

    #[test]
    fn refuses_an_unreadable_region() {
        let error = replace_table(
            &article("<p>edited</p>"),
            &table(&[("8.1", Some("8.1.1"))]),
            floor(),
        )
        .unwrap_err();

        assert!(
            matches!(error, UpdateError::CurrentTable { .. }),
            "got {error}"
        );
    }

    #[test]
    fn refuses_missing_or_repeated_markers() {
        let generated = table(&[("8.1", Some("8.1.1"))]);
        let articles = [
            "no markers here".to_string(),
            article("").replace("<!-- END GENERATED ruby_versions_table -->", ""),
            format!("{}<!-- BEGIN GENERATED ruby_versions_table -->", article("")),
            "<!-- END GENERATED ruby_versions_table -->\n<!-- BEGIN GENERATED ruby_versions_table -->"
                .to_string(),
        ];
        for content in articles {
            let error = replace_table(&content, &generated, floor()).unwrap_err();
            assert!(
                matches!(error, UpdateError::Markers { .. }),
                "{content:?} got {error}"
            );
        }
    }
}
