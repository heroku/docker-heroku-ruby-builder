//! One row per JRuby `MAJOR.MINOR` line, newest first, showing a release of that line and where it is available.

use crate::guard::RowVersions;
use crate::update::VersionTable;
use crate::{CheckError, MajorMinor, ReadError, TableText, UNAVAILABLE, join_all, render};
use jruby_executable::github_releases::s3_urls_to_check;
use jruby_executable::{JRubyVersion, jruby_build_properties};
use minijinja::{Value, context};
use shared::github::GitHubToken;
use shared::{BaseImage, RubyDownloadVersion, s3_presence};
use std::collections::BTreeSet;
use tokio::task::JoinSet;

/// Shown in a cell when the row's release is available on that base image.
const AVAILABLE: &str = "✅";

/// A JRuby release and the base images it is built for on every architecture.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JRubyBuild {
    pub version: JRubyVersion,
    /// The Ruby standard library version this release implements.
    pub ruby_compat: RubyDownloadVersion,
    pub base_images: BTreeSet<BaseImage>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JRubyTable {
    pub base_images: Vec<BaseImage>,
    pub rows: Vec<JRubyBuild>,
}

/// Checks S3 for every release at or above `floor`.
pub async fn builds(
    releases: &[JRubyVersion],
    floor: MajorMinor,
    gh_token: &GitHubToken,
) -> Result<Vec<JRubyBuild>, CheckError> {
    let mut set = JoinSet::new();
    for version in releases
        .iter()
        .filter(|version| MajorMinor::from(*version) >= floor)
        .cloned()
    {
        let gh_token = gh_token.clone();
        set.spawn(async move {
            let stdlib = jruby_build_properties(&version, &gh_token)
                .await?
                .ruby_stdlib_version()?;
            let presence = s3_presence(s3_urls_to_check(&version, &stdlib)).await?;
            Ok(JRubyBuild {
                version,
                ruby_compat: RubyDownloadVersion::new(&stdlib)?,
                base_images: presence.complete_base_images().into_iter().collect(),
            })
        });
    }
    join_all(set).await
}

impl JRubyTable {
    /// A row for every line at or above `floor` with a release available somewhere.
    ///
    /// Each row is the newest release of its line that is available on every base image where the next older
    /// available release is, so a release that is not fully built yet does not replace one that is.
    pub fn new(
        builds: &[JRubyBuild],
        floor: MajorMinor,
        base_images: Vec<BaseImage>,
    ) -> JRubyTable {
        let mut builds = builds
            .iter()
            .filter(|build| {
                MajorMinor::from(&build.version) >= floor && !build.base_images.is_empty()
            })
            .collect::<Vec<_>>();
        builds.sort_by(|a, b| b.version.cmp(&a.version));
        builds.dedup_by(|a, b| a.version == b.version);

        let rows = builds
            .chunk_by(|a, b| MajorMinor::from(&a.version) == MajorMinor::from(&b.version))
            .map(|line| {
                let mut row = line[0];
                for &older in &line[1..] {
                    if older.base_images.is_subset(&row.base_images) {
                        break;
                    }
                    row = older;
                }
                row.clone()
            })
            .collect();
        JRubyTable { base_images, rows }
    }
}

impl VersionTable for JRubyTable {
    const NAME: &'static str = "jruby_versions_table";

    type Version = JRubyVersion;

    fn render(&self) -> String {
        let rows = self
            .rows
            .iter()
            .map(|row| {
                context! {
                    version => row.version.to_string(),
                    ruby_compat => row.ruby_compat.to_string(),
                    built => self
                        .base_images
                        .iter()
                        .map(|base_image| row.base_images.contains(base_image))
                        .collect::<Value>(),
                }
            })
            .collect::<Value>();
        render(
            "jruby_versions_table.html",
            include_str!("../templates/jruby_versions_table.html"),
            context! {
                base_images => self.base_images.iter().map(ToString::to_string).collect::<Value>(),
                rows,
                available => AVAILABLE,
                unavailable => UNAVAILABLE,
            },
        )
    }

    fn read(html: &str) -> Result<JRubyTable, ReadError> {
        let TableText { base_images, rows } =
            TableText::read(html, &["JRuby version", "Ruby compatibility"])?;
        let rows = rows
            .iter()
            .map(|cells| {
                let [version, ruby_compat, marks @ ..] = &cells[..] else {
                    unreachable!("every row has a cell per column")
                };
                let version = JRubyVersion::parse(version).map_err(|_| ReadError::Cell {
                    expected: "a JRuby version",
                    found: version.clone(),
                })?;
                let ruby_compat =
                    RubyDownloadVersion::new(ruby_compat).map_err(|_| ReadError::Cell {
                        expected: "a Ruby version",
                        found: ruby_compat.clone(),
                    })?;
                let mut built = BTreeSet::new();
                for (base_image, mark) in base_images.iter().zip(marks) {
                    match mark.as_str() {
                        AVAILABLE => {
                            built.insert(base_image.clone());
                        }
                        UNAVAILABLE => {}
                        _ => {
                            return Err(ReadError::Cell {
                                expected: "an availability mark",
                                found: mark.clone(),
                            });
                        }
                    }
                }
                Ok(JRubyBuild {
                    version,
                    ruby_compat,
                    base_images: built,
                })
            })
            .collect::<Result<_, _>>()?;
        Ok(JRubyTable { base_images, rows })
    }

    fn base_images(&self) -> &[BaseImage] {
        &self.base_images
    }

    /// An available cell shows the row's release.
    fn row_versions(&self) -> Vec<RowVersions<JRubyVersion>> {
        self.rows
            .iter()
            .map(|row| RowVersions {
                major_minor: MajorMinor::from(&row.version),
                versions: row
                    .base_images
                    .iter()
                    .map(|base_image| (base_image.clone(), row.version.clone()))
                    .collect(),
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use indoc::indoc;
    use pretty_assertions::assert_eq;

    fn base_images() -> Vec<BaseImage> {
        ["heroku-24", "heroku-26"]
            .map(|name| BaseImage::new(name).unwrap())
            .to_vec()
    }

    fn build(version: &str, base_images: &[&str]) -> JRubyBuild {
        JRubyBuild {
            version: JRubyVersion::parse(version).unwrap(),
            ruby_compat: RubyDownloadVersion::new("8.1.0").unwrap(),
            base_images: base_images
                .iter()
                .map(|name| BaseImage::new(name).unwrap())
                .collect(),
        }
    }

    fn versions(table: &JRubyTable) -> Vec<String> {
        table
            .rows
            .iter()
            .map(|row| row.version.to_string())
            .collect()
    }

    #[test]
    fn keeps_a_release_until_a_newer_one_is_built_everywhere_it_is() {
        let builds = [
            // In this fixture 20.0.3.0 is built on heroku-24 only, so 20.0.2.0 stays.
            build("20.0.3.0", &["heroku-24"]),
            build("20.0.2.0", &["heroku-24", "heroku-26"]),
            build("20.0.1.0", &["heroku-24", "heroku-26"]),
            // 19.9.2.0 is built everywhere 19.9.1.0 is, so it replaces it.
            build("19.9.2.0", &["heroku-24"]),
            build("19.9.1.0", &["heroku-24"]),
            build("19.8.0.0", &["heroku-24", "heroku-26"]),
        ];

        let table = JRubyTable::new(&builds, "19.9".parse().unwrap(), base_images());

        assert_eq!(versions(&table), vec!["20.0.2.0", "19.9.2.0"]);
    }

    #[test]
    fn shows_a_newer_release_once_it_is_built_everywhere() {
        let builds = [
            build("20.0.3.0", &["heroku-24", "heroku-26"]),
            build("20.0.2.0", &["heroku-24", "heroku-26"]),
        ];

        let table = JRubyTable::new(&builds, "19.9".parse().unwrap(), base_images());

        assert_eq!(versions(&table), vec!["20.0.3.0"]);
    }

    #[test]
    fn skips_an_unbuilt_release() {
        let builds = [build("20.0.3.0", &[]), build("20.0.2.0", &["heroku-24"])];

        let table = JRubyTable::new(&builds, "19.9".parse().unwrap(), base_images());

        assert_eq!(versions(&table), vec!["20.0.2.0"]);
    }

    #[test]
    fn leaves_out_a_line_with_nothing_available() {
        let table = JRubyTable::new(
            &[build("20.1.0.0", &[])],
            "19.9".parse().unwrap(),
            base_images(),
        );

        assert_eq!(versions(&table), Vec::<String>::new());
    }

    fn rendered_fixture() -> JRubyTable {
        JRubyTable::new(
            &[
                build("20.0.2.0", &["heroku-24", "heroku-26"]),
                build("19.9.2.0", &["heroku-24"]),
            ],
            "19.9".parse().unwrap(),
            base_images(),
        )
    }

    #[test]
    fn renders_one_tag_per_line_with_availability_marks() {
        assert_eq!(
            rendered_fixture().render(),
            indoc! {"
                <table>
                <thead>
                <tr>
                <th>JRuby version</th>
                <th>Ruby compatibility</th>
                <th>heroku-24</th>
                <th>heroku-26</th>
                </tr>
                </thead>
                <tbody>
                <tr>
                <td>20.0.2.0</td>
                <td>8.1.0</td>
                <td>✅</td>
                <td>✅</td>
                </tr>
                <tr>
                <td>19.9.2.0</td>
                <td>8.1.0</td>
                <td>✅</td>
                <td>–</td>
                </tr>
                </tbody>
                </table>"}
        );
    }

    #[test]
    fn reads_what_it_renders() {
        let table = rendered_fixture();
        assert_eq!(JRubyTable::read(&table.render()), Ok(table));
    }

    #[test]
    fn rejects_an_unrecognized_mark() {
        let html = rendered_fixture()
            .render()
            .replacen("<td>–</td>", "<td>soon</td>", 1);
        assert_eq!(
            JRubyTable::read(&html),
            Err(ReadError::Cell {
                expected: "an availability mark",
                found: "soon".to_string()
            })
        );
    }
}
