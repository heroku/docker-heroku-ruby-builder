//! One row per JRuby minor line, newest first, showing the newest release of that line and where it is available.

use crate::guard::{self, GuardRow};
use crate::html_table::{Row, Table};
use crate::{Series, Stack, UNAVAILABLE, all_exist};
use jruby_executable::JRubyVersion;
use jruby_executable::github_releases::s3_urls_to_check;
use url::Url;

pub const NAME: &str = "jruby_versions_table";

/// Shown in a cell when the row's release is available on that stack.
const AVAILABLE: &str = "✅";

#[derive(Debug, Clone)]
pub struct JRubyRow {
    pub version: JRubyVersion,
    /// The Ruby standard library version this release implements, such as `3.4.5`.
    pub ruby_compat: String,
    /// Whether the release is available, one entry per stack.
    pub cells: Vec<bool>,
}

fn series(version: &JRubyVersion) -> Series {
    Series {
        major: version.major(),
        minor: version.minor(),
    }
}

fn tar_urls(version: &JRubyVersion, ruby_stdlib: &str, stack: &Stack) -> Vec<Url> {
    s3_urls_to_check(version, ruby_stdlib)
        .into_iter()
        .filter(|(_, base_image, _)| base_image.name() == stack.base_image.name())
        .map(|(url, _, _)| url)
        .collect()
}

/// Builds a row for every JRuby minor line at or above `floor`.
///
/// Each row is the newest release of its line that is available on at least one stack, so a release that is not
/// built yet does not replace one that is. A line with nothing available is left out.
pub async fn collect<S, SFut, F, Fut, E>(
    releases: &[JRubyVersion],
    floor: Series,
    stacks: &[Stack],
    ruby_stdlib: S,
    exists: F,
) -> Result<Vec<JRubyRow>, E>
where
    S: Fn(JRubyVersion) -> SFut,
    SFut: Future<Output = Result<String, E>>,
    F: Fn(Url) -> Fut,
    Fut: Future<Output = Result<bool, E>>,
{
    let mut releases = releases
        .iter()
        .filter(|version| series(version) >= floor)
        .cloned()
        .collect::<Vec<_>>();
    releases.sort_by(|a, b| b.cmp(a));
    releases.dedup();

    let mut rows: Vec<JRubyRow> = Vec::new();
    for version in releases {
        if rows
            .last()
            .is_some_and(|row| series(&row.version) == series(&version))
        {
            continue;
        }
        let ruby_compat = ruby_stdlib(version.clone()).await?;
        let mut cells = Vec::new();
        for stack in stacks {
            cells.push(all_exist(tar_urls(&version, &ruby_compat, stack), &exists).await?);
        }
        if cells.iter().any(|available| *available) {
            rows.push(JRubyRow {
                version,
                ruby_compat,
                cells,
            });
        }
    }
    Ok(rows)
}

pub fn render(rows: &[JRubyRow], stacks: &[Stack]) -> Table {
    let mut headers = vec![
        "JRuby version".to_string(),
        "Ruby compatibility".to_string(),
    ];
    headers.extend(stacks.iter().map(|stack| stack.base_image.to_string()));
    Table {
        headers,
        rows: rows
            .iter()
            .map(|row| {
                let mut cells = vec![row.version.to_string(), row.ruby_compat.clone()];
                cells.extend(
                    row.cells.iter().map(|available| {
                        if *available { AVAILABLE } else { UNAVAILABLE }.to_string()
                    }),
                );
                Row { style: None, cells }
            })
            .collect(),
    }
}

/// Reads a rendered JRuby table back into comparable rows for the regression guard. An available cell compares as
/// the row's JRuby version.
pub fn guard_rows(table: &Table) -> Result<Vec<GuardRow>, guard::RowError> {
    let stacks = table.headers.get(2..).unwrap_or_default();
    table
        .rows
        .iter()
        .map(|row| {
            let [version, _, cells @ ..] = &row.cells[..] else {
                return Err(guard::RowError::Empty);
            };
            let parsed = JRubyVersion::parse(version)
                .map_err(|_| guard::RowError::Version(version.clone()))?;
            GuardRow::new(
                series(&parsed),
                stacks,
                cells
                    .iter()
                    .map(|cell| (cell == AVAILABLE).then_some(version.as_str())),
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::bucket;
    use pretty_assertions::assert_eq;

    fn versions(list: &[&str]) -> Vec<JRubyVersion> {
        list.iter()
            .map(|v| JRubyVersion::parse(v).unwrap())
            .collect()
    }

    fn stdlib(version: JRubyVersion) -> std::future::Ready<Result<String, String>> {
        std::future::ready(Ok(match series(&version).major {
            10 => "3.4.5",
            _ => "3.1.7",
        }
        .to_string()))
    }

    fn summary(rows: &[JRubyRow]) -> Vec<String> {
        rows.iter()
            .map(|row| format!("{} ({}): {:?}", row.version, row.ruby_compat, row.cells))
            .collect()
    }

    #[test]
    fn checks_every_arch_of_the_stack() {
        let version = JRubyVersion::parse("9.4.15.0").unwrap();
        let stacks = crate::stacks();
        let urls = tar_urls(&version, "3.1.7", &stacks[1])
            .into_iter()
            .map(|url| url.path().to_string())
            .collect::<Vec<_>>();
        assert_eq!(
            urls,
            vec![
                "/heroku-24/amd64/ruby-3.1.7-jruby-9.4.15.0.tgz",
                "/heroku-24/arm64/ruby-3.1.7-jruby-9.4.15.0.tgz",
            ]
        );
    }

    #[tokio::test]
    async fn shows_the_newest_available_release_of_each_line() {
        let releases = versions(&["9.3.15.0", "9.4.14.0", "9.4.15.0", "10.0.6.0", "10.0.7.0"]);
        let s3 = bucket(&[
            // 10.0.7.0 is not built anywhere yet, so 10.0.6.0 stays.
            "heroku-22/amd64/ruby-3.4.5-jruby-10.0.6.0.tgz",
            "heroku-24/amd64/ruby-3.4.5-jruby-10.0.6.0.tgz",
            "heroku-24/arm64/ruby-3.4.5-jruby-10.0.6.0.tgz",
            "heroku-22/amd64/ruby-3.1.7-jruby-9.4.15.0.tgz",
            // Only one heroku-26 arch, so heroku-26 is unavailable.
            "heroku-26/amd64/ruby-3.1.7-jruby-9.4.15.0.tgz",
            "heroku-22/amd64/ruby-3.1.7-jruby-9.3.15.0.tgz",
        ]);

        let rows = collect(
            &releases,
            "9.4".parse().unwrap(),
            &crate::stacks(),
            stdlib,
            s3,
        )
        .await
        .unwrap();

        assert_eq!(
            summary(&rows),
            vec![
                "10.0.6.0 (3.4.5): [true, true, false]",
                "9.4.15.0 (3.1.7): [true, false, false]",
            ]
        );
    }

    #[tokio::test]
    async fn leaves_out_a_line_with_nothing_available() {
        let releases = versions(&["10.1.0.0"]);

        let rows = collect(
            &releases,
            "9.4".parse().unwrap(),
            &crate::stacks(),
            stdlib,
            bucket(&[]),
        )
        .await
        .unwrap();

        assert!(rows.is_empty(), "{:?}", summary(&rows));
    }

    #[test]
    fn renders_availability_and_reads_it_back() {
        let rows = [JRubyRow {
            version: JRubyVersion::parse("10.0.7.0").unwrap(),
            ruby_compat: "3.4.5".to_string(),
            cells: vec![true, false, true],
        }];

        let table = render(&rows, &crate::stacks());

        assert_eq!(
            table.headers,
            vec![
                "JRuby version",
                "Ruby compatibility",
                "heroku-22",
                "heroku-24",
                "heroku-26"
            ]
        );
        assert_eq!(
            table.rows,
            vec![Row {
                style: None,
                cells: vec!["10.0.7.0", "3.4.5", "✅", "–", "✅"]
                    .into_iter()
                    .map(String::from)
                    .collect(),
            }]
        );

        let stacks = ["heroku-22", "heroku-24", "heroku-26"].map(String::from);
        assert_eq!(
            guard_rows(&table).unwrap(),
            vec![
                GuardRow::new(
                    "10.0".parse().unwrap(),
                    &stacks,
                    [Some("10.0.7.0"), None, Some("10.0.7.0")].into_iter()
                )
                .unwrap()
            ]
        );
    }
}
