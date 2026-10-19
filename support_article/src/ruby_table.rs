//! One row per Ruby minor series, newest first, with the newest patch available on each stack.

use crate::guard::{self, GuardRow};
use crate::html_table::{Row, Table};
use crate::{Series, Stack, UNAVAILABLE, all_exist, s3_url};
use ruby_executable::ruby_lang::{BranchStatus, RubyBranch};
use shared::{RubyDownloadVersion, output_ruby_tar_path};
use std::path::Path;
use url::Url;

pub const NAME: &str = "ruby_versions_table";

/// Upstream maintenance status of a series shown in the table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lifecycle {
    Maintained,
    SecurityOnly,
    Eol,
}

#[derive(Debug, Clone)]
pub struct RubySeriesRow {
    pub series: Series,
    pub lifecycle: Lifecycle,
    /// The newest available patch, one entry per stack.
    pub cells: Vec<Option<RubyDownloadVersion>>,
}

/// Where `version` lives on S3 for every architecture of `stack`.
///
/// `heroku-22` is checked at its legacy path without an architecture directory. Older `heroku-22` builds exist only
/// there.
fn tar_urls(version: &RubyDownloadVersion, stack: &Stack) -> Vec<Url> {
    let archs = if stack.base_image.has_legacy_path() {
        vec![None]
    } else {
        stack.archs.iter().map(Some).collect()
    };
    archs
        .into_iter()
        .map(|arch| {
            s3_url(&output_ruby_tar_path(
                Path::new(""),
                version,
                &stack.base_image,
                arch,
            ))
        })
        .collect()
}

/// Builds the rows for every non-preview series at or above `floor`.
///
/// Each cell walks the series' stable releases newest first and stops at the first one present on S3. A series with
/// nothing available on any stack is left out.
pub async fn collect<F, Fut, E>(
    branches: &[RubyBranch],
    releases: &[RubyDownloadVersion],
    floor: Series,
    stacks: &[Stack],
    exists: F,
) -> Result<Vec<RubySeriesRow>, E>
where
    F: Fn(Url) -> Fut,
    Fut: Future<Output = Result<bool, E>>,
{
    let mut branches = branches
        .iter()
        .filter_map(|branch| {
            let lifecycle = match branch.status {
                BranchStatus::NormalMaintenance => Lifecycle::Maintained,
                BranchStatus::SecurityMaintenance => Lifecycle::SecurityOnly,
                BranchStatus::Eol => Lifecycle::Eol,
                BranchStatus::Preview => return None,
            };
            let series = Series {
                major: branch.major,
                minor: branch.minor,
            };
            (series >= floor).then_some((series, lifecycle))
        })
        .collect::<Vec<_>>();
    branches.sort_by_key(|(series, _)| std::cmp::Reverse(*series));

    let mut rows = Vec::new();
    for (series, lifecycle) in branches {
        let mut patches = releases
            .iter()
            .filter(|version| {
                version.major == series.major
                    && version.minor == series.minor
                    && version.is_prerelease().is_none()
            })
            .collect::<Vec<_>>();
        patches.sort_by_key(|version| std::cmp::Reverse(version.patch));
        patches.dedup_by_key(|version| version.patch);

        let mut cells = Vec::new();
        for stack in stacks {
            let mut newest = None;
            for patch in &patches {
                if all_exist(tar_urls(patch, stack), &exists).await? {
                    newest = Some((*patch).clone());
                    break;
                }
            }
            cells.push(newest);
        }
        if cells.iter().any(Option::is_some) {
            rows.push(RubySeriesRow {
                series,
                lifecycle,
                cells,
            });
        }
    }
    Ok(rows)
}

pub fn render(rows: &[RubySeriesRow], stacks: &[Stack]) -> Table {
    let mut headers = vec!["Ruby series".to_string()];
    headers.extend(stacks.iter().map(|stack| stack.base_image.to_string()));
    Table {
        headers,
        rows: rows
            .iter()
            .map(|row| {
                let (suffix, style) = match row.lifecycle {
                    Lifecycle::Maintained => ("", None),
                    Lifecycle::SecurityOnly => (
                        " (security-only)",
                        Some("background-color:#fcf8e3;color:#8a6d3b;"),
                    ),
                    Lifecycle::Eol => (" (EOL)", Some("background-color:#f2dede;color:#a94442;")),
                };
                let mut cells = vec![format!("{}{suffix}", row.series)];
                cells.extend(row.cells.iter().map(|cell| match cell {
                    Some(version) => version.to_string(),
                    None => UNAVAILABLE.to_string(),
                }));
                Row {
                    style: style.map(String::from),
                    cells,
                }
            })
            .collect(),
    }
}

/// Reads a rendered Ruby table back into comparable rows for the regression guard.
pub fn guard_rows(table: &Table) -> Result<Vec<GuardRow>, guard::RowError> {
    let stacks = table.headers.get(1..).unwrap_or_default();
    table
        .rows
        .iter()
        .map(|row| {
            let (label, cells) = row.cells.split_first().ok_or(guard::RowError::Empty)?;
            let series = label
                .split_whitespace()
                .next()
                .unwrap_or_default()
                .parse::<Series>()?;
            GuardRow::new(
                series,
                stacks,
                cells
                    .iter()
                    .map(|cell| (cell != UNAVAILABLE).then_some(cell.as_str())),
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::bucket;
    use pretty_assertions::assert_eq;

    fn branch(name: &str, status: BranchStatus) -> RubyBranch {
        let series: Series = name.parse().unwrap();
        RubyBranch {
            major: series.major,
            minor: series.minor,
            status,
        }
    }

    fn versions(list: &[&str]) -> Vec<RubyDownloadVersion> {
        list.iter()
            .map(|v| RubyDownloadVersion::new(v).unwrap())
            .collect()
    }

    fn cells(rows: &[RubySeriesRow]) -> Vec<String> {
        rows.iter()
            .map(|row| {
                let cells = row
                    .cells
                    .iter()
                    .map(|cell| {
                        cell.as_ref()
                            .map_or(UNAVAILABLE.to_string(), ToString::to_string)
                    })
                    .collect::<Vec<_>>()
                    .join(" ");
                format!("{} {:?}: {cells}", row.series, row.lifecycle)
            })
            .collect()
    }

    #[test]
    fn checks_heroku_22_at_the_legacy_path_and_newer_stacks_per_arch() {
        let version = RubyDownloadVersion::new("3.4.1").unwrap();
        let urls = crate::stacks()
            .iter()
            .flat_map(|stack| tar_urls(&version, stack))
            .map(|url| url.path().to_string())
            .collect::<Vec<_>>();
        assert_eq!(
            urls,
            vec![
                "/heroku-22/ruby-3.4.1.tgz",
                "/heroku-24/amd64/ruby-3.4.1.tgz",
                "/heroku-24/arm64/ruby-3.4.1.tgz",
                "/heroku-26/amd64/ruby-3.4.1.tgz",
                "/heroku-26/arm64/ruby-3.4.1.tgz",
            ]
        );
    }

    #[tokio::test]
    async fn picks_the_newest_patch_present_on_every_arch_of_each_stack() {
        let branches = [
            branch("4.1", BranchStatus::Preview),
            branch("3.3", BranchStatus::SecurityMaintenance),
            branch("3.4", BranchStatus::NormalMaintenance),
            branch("3.1", BranchStatus::Eol),
            branch("3.0", BranchStatus::Eol),
        ];
        let releases = versions(&[
            "4.1.0-preview1",
            "3.4.2",
            "3.4.1",
            "3.4.0",
            "3.4.0-rc1",
            "3.3.9",
            "3.1.7",
            "3.0.7",
        ]);
        let s3 = bucket(&[
            "heroku-22/ruby-4.1.0.preview1.tgz",
            "heroku-22/ruby-3.4.2.tgz",
            // 3.4.2 is only on one heroku-24 arch, so heroku-24 falls back to 3.4.1.
            "heroku-24/amd64/ruby-3.4.2.tgz",
            "heroku-24/amd64/ruby-3.4.1.tgz",
            "heroku-24/arm64/ruby-3.4.1.tgz",
            "heroku-26/amd64/ruby-3.4.0.tgz",
            "heroku-26/arm64/ruby-3.4.0.tgz",
            "heroku-22/ruby-3.3.9.tgz",
            "heroku-24/amd64/ruby-3.3.9.tgz",
            "heroku-24/arm64/ruby-3.3.9.tgz",
            "heroku-26/amd64/ruby-3.3.9.tgz",
            "heroku-26/arm64/ruby-3.3.9.tgz",
            "heroku-22/ruby-3.1.7.tgz",
            "heroku-22/ruby-3.0.7.tgz",
        ]);

        let rows = collect(
            &branches,
            &releases,
            "3.1".parse().unwrap(),
            &crate::stacks(),
            s3,
        )
        .await
        .unwrap();

        assert_eq!(
            cells(&rows),
            vec![
                "3.4 Maintained: 3.4.2 3.4.1 3.4.0",
                "3.3 SecurityOnly: 3.3.9 3.3.9 3.3.9",
                "3.1 Eol: 3.1.7 – –",
            ]
        );
    }

    #[tokio::test]
    async fn leaves_out_a_series_with_nothing_available() {
        let branches = [branch("4.0", BranchStatus::NormalMaintenance)];
        let releases = versions(&["4.0.0"]);

        let rows = collect(
            &branches,
            &releases,
            "3.1".parse().unwrap(),
            &crate::stacks(),
            bucket(&[]),
        )
        .await
        .unwrap();

        assert!(rows.is_empty(), "{:?}", cells(&rows));
    }

    #[tokio::test]
    async fn stops_at_the_first_available_patch() {
        let branches = [branch("3.4", BranchStatus::NormalMaintenance)];
        let releases = versions(&["3.4.1", "3.4.0"]);
        let checked = std::sync::Mutex::new(Vec::new());
        let s3 = |url: Url| {
            checked.lock().unwrap().push(url.path().to_string());
            std::future::ready(Ok::<_, String>(true))
        };

        collect(
            &branches,
            &releases,
            "3.1".parse().unwrap(),
            &crate::stacks(),
            s3,
        )
        .await
        .unwrap();

        assert!(
            checked
                .lock()
                .unwrap()
                .iter()
                .all(|path| path.contains("3.4.1")),
            "{checked:?}"
        );
    }

    #[test]
    fn renders_lifecycle_labels_and_styles() {
        let stacks = crate::stacks();
        let row = |series: &str, lifecycle, cells: &[Option<&str>]| RubySeriesRow {
            series: series.parse().unwrap(),
            lifecycle,
            cells: cells
                .iter()
                .map(|cell| cell.map(|v| RubyDownloadVersion::new(v).unwrap()))
                .collect(),
        };
        let table = render(
            &[
                row("3.4", Lifecycle::Maintained, &[Some("3.4.1"); 3]),
                row("3.3", Lifecycle::SecurityOnly, &[Some("3.3.9"); 3]),
                row("3.1", Lifecycle::Eol, &[Some("3.1.7"), Some("3.1.7"), None]),
            ],
            &stacks,
        );

        assert_eq!(
            table.headers,
            vec!["Ruby series", "heroku-22", "heroku-24", "heroku-26"]
        );
        assert_eq!(
            table.rows,
            vec![
                Row {
                    style: None,
                    cells: vec!["3.4", "3.4.1", "3.4.1", "3.4.1"]
                        .into_iter()
                        .map(String::from)
                        .collect()
                },
                Row {
                    style: Some("background-color:#fcf8e3;color:#8a6d3b;".to_string()),
                    cells: vec!["3.3 (security-only)", "3.3.9", "3.3.9", "3.3.9"]
                        .into_iter()
                        .map(String::from)
                        .collect()
                },
                Row {
                    style: Some("background-color:#f2dede;color:#a94442;".to_string()),
                    cells: vec!["3.1 (EOL)", "3.1.7", "3.1.7", "–"]
                        .into_iter()
                        .map(String::from)
                        .collect()
                },
            ]
        );

        let guard_rows = guard_rows(&table).unwrap();
        assert_eq!(
            guard_rows
                .iter()
                .map(|row| row.series.to_string())
                .collect::<Vec<_>>(),
            vec!["3.4", "3.3", "3.1"]
        );
    }
}
