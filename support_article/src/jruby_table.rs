//! One row per JRuby `MAJOR.MINOR` line, newest first, with the newest release available on each base image.

use crate::{CheckError, join_all};
use jruby_executable::github_releases::s3_urls_to_check;
use jruby_executable::{JRubyVersion, jruby_build_properties};
use minijinja::{Value, context};
use shared::github::GitHubToken;
use shared::{BaseImage, MajorMinor, RubyDownloadVersion, s3_presence};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::{self, Display, Formatter};
use tokio::task::JoinSet;

/// A JRuby version and the Ruby standard library version it implements.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JRubyRelease {
    pub version: JRubyVersion,
    pub ruby_compat: RubyDownloadVersion,
}

/// Formats as `JRuby version / Ruby version`.
impl Display for JRubyRelease {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{} / {}", self.version, self.ruby_compat)
    }
}

/// A JRuby release and the base images it is built for on every architecture.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JRubyBuild {
    pub release: JRubyRelease,
    pub base_images: BTreeSet<BaseImage>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JRubyRow {
    pub major_minor: MajorMinor,
    /// The newest release available on each base image.
    pub releases: BTreeMap<BaseImage, JRubyRelease>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JRubyTable {
    pub rows: Vec<JRubyRow>,
    /// The release shown in the Gemfile example: the newest one on the newest base image.
    pub example: JRubyRelease,
}

/// An empty table means an upstream source or S3 lookup is broken, so it is not published.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("no JRuby release at or above {floor} is available on any base image")]
pub struct EmptyTable {
    pub floor: MajorMinor,
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
                release: JRubyRelease {
                    version,
                    ruby_compat: RubyDownloadVersion::new(&stdlib)?,
                },
                base_images: presence.complete_base_images().into_iter().collect(),
            })
        });
    }
    join_all(set).await
}

impl JRubyTable {
    /// A row for every line at or above `floor` with a release available somewhere.
    pub fn new(builds: &[JRubyBuild], floor: MajorMinor) -> Result<JRubyTable, EmptyTable> {
        let mut lines = BTreeMap::<MajorMinor, BTreeMap<BaseImage, JRubyRelease>>::new();
        for build in builds
            .iter()
            .filter(|build| MajorMinor::from(&build.release.version) >= floor)
        {
            for base_image in &build.base_images {
                let newest = lines
                    .entry(MajorMinor::from(&build.release.version))
                    .or_default();
                if newest
                    .get(base_image)
                    .is_none_or(|newest| build.release.version > newest.version)
                {
                    newest.insert(base_image.clone(), build.release.clone());
                }
            }
        }

        let rows = lines
            .into_iter()
            .rev()
            .map(|(major_minor, releases)| JRubyRow {
                major_minor,
                releases,
            })
            .collect::<Vec<_>>();
        let (_, example) = rows
            .iter()
            .flat_map(|row| &row.releases)
            .max_by_key(|&(base_image, release)| (base_image, &release.version))
            .ok_or(EmptyTable { floor })?;
        Ok(JRubyTable {
            example: example.clone(),
            rows,
        })
    }

    /// The table's columns.
    pub fn base_images(&self) -> BTreeSet<&BaseImage> {
        self.rows
            .iter()
            .flat_map(|row| row.releases.keys())
            .collect()
    }
}

impl JRubyTable {
    /// Markdown with `\n` line endings: the HTML table, then a Gemfile example.
    pub fn render(&self) -> Result<String, minijinja::Error> {
        crate::render_template(
            "jruby_versions_table.md.jinja",
            include_str!("../templates/jruby_versions_table.md.jinja"),
            context! {
                base_images => self.base_images().iter().map(ToString::to_string).collect::<Value>(),
                rows => self.render_row_values(),
                example => context! {
                    version => self.example.version.to_string(),
                    ruby_compat => self.example.ruby_compat.to_string(),
                },
            },
        )
    }

    fn render_row_values(&self) -> Value {
        self.rows
            .iter()
            .map(|row| {
                context! {
                    major_minor => row.major_minor.to_string(),
                    releases => self.base_images()
                        .iter()
                        .map(|base_image| row.releases.get(base_image).map(ToString::to_string))
                        .collect::<Value>(),
                }
            })
            .collect::<Value>()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn build(version: &str, base_images: &[&str]) -> JRubyBuild {
        JRubyBuild {
            release: JRubyRelease {
                version: JRubyVersion::parse(version).unwrap(),
                ruby_compat: RubyDownloadVersion::new("8.1.0").unwrap(),
            },
            base_images: base_images
                .iter()
                .map(|name| BaseImage::new(name).unwrap())
                .collect(),
        }
    }

    fn with_ruby_compat(ruby_compat: &str, mut build: JRubyBuild) -> JRubyBuild {
        build.release.ruby_compat = RubyDownloadVersion::new(ruby_compat).unwrap();
        build
    }

    fn summary(table: &JRubyTable) -> Vec<String> {
        table
            .rows
            .iter()
            .map(|row| {
                let cells = table
                    .base_images()
                    .iter()
                    .map(|base_image| {
                        row.releases
                            .get(base_image)
                            .map_or("-".to_string(), ToString::to_string)
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("{}: {cells}", row.major_minor)
            })
            .collect()
    }

    #[test]
    fn shows_the_newest_release_available_on_each_base_image() {
        let builds = [
            build("19.9.1.0", &["heroku-24"]),
            build("20.0.2.0", &["heroku-24", "heroku-26"]),
            with_ruby_compat("8.1.1", build("20.0.10.0", &["heroku-24"])),
            build("19.9.2.0", &["heroku-24"]),
            build("20.0.11.0", &[]),
            build("20.0.9.0", &["heroku-24", "heroku-26"]),
            build("19.8.0.0", &["heroku-24", "heroku-26"]),
        ];

        let table = JRubyTable::new(&builds, "19.9".parse().unwrap()).unwrap();

        insta::assert_snapshot!(summary(&table).join("\n"), @r"
        20.0: 20.0.10.0 / 8.1.1, 20.0.9.0 / 8.1.0
        19.9: 19.9.2.0 / 8.1.0, -
        ");
    }

    #[test]
    fn leaves_out_a_line_with_nothing_available() {
        let table = JRubyTable::new(
            &[build("20.1.0.0", &[]), build("20.0.2.0", &["heroku-24"])],
            "19.9".parse().unwrap(),
        )
        .unwrap();

        assert_eq!(summary(&table), vec!["20.0: 20.0.2.0 / 8.1.0"]);
    }

    #[test]
    fn errors_when_nothing_is_available() {
        let error =
            JRubyTable::new(&[build("20.1.0.0", &[])], "19.9".parse().unwrap()).unwrap_err();

        assert_eq!(
            error.to_string(),
            "no JRuby release at or above 19.9 is available on any base image"
        );
    }

    #[test]
    fn leaves_out_a_base_image_with_nothing_available() {
        let table = JRubyTable::new(
            &[build("20.0.2.0", &["heroku-24"])],
            "19.9".parse().unwrap(),
        )
        .unwrap();

        assert_eq!(summary(&table), vec!["20.0: 20.0.2.0 / 8.1.0"]);
    }

    #[test]
    fn example_is_the_newest_release_on_the_newest_base_image() {
        let table = JRubyTable::new(
            &[
                build("20.1.0.0", &["heroku-24"]),
                build("20.0.2.0", &["heroku-24", "heroku-26"]),
                build("19.9.3.0", &["heroku-26"]),
            ],
            "19.9".parse().unwrap(),
        )
        .unwrap();

        assert_eq!(table.example.version.to_string(), "20.0.2.0");
    }

    fn rendered_fixture() -> JRubyTable {
        JRubyTable::new(
            &[
                build("20.0.2.0", &["heroku-24", "heroku-26"]),
                build("19.9.2.0", &["heroku-24"]),
            ],
            "19.9".parse().unwrap(),
        )
        .unwrap()
    }

    #[test]
    fn renders_the_table_and_a_gemfile_example() {
        insta::assert_snapshot!(rendered_fixture().render().unwrap(), @r"
        <table>
          <thead>
            <tr>
              <th>JRuby series</th>
                <th>heroku-24</th>
                <th>heroku-26</th>
            </tr>
          </thead>
          <tbody>
              <tr>
                <td>20.0</td>
                  <td>20.0.2.0 / 8.1.0</td>
                  <td>20.0.2.0 / 8.1.0</td>
              </tr>
              <tr>
                <td>19.9</td>
                  <td>19.9.2.0 / 8.1.0</td>
                  <td>-</td>
              </tr>
          </tbody>
        </table>

        To use a JRuby version, specify it in your `Gemfile`. For example:

        ```ruby
        # Gemfile
        ruby '8.1.0', engine: 'jruby', engine_version: '20.0.2.0'
        ```

        Then, with that JRuby version installed locally, update your `Gemfile.lock` by running:

        ```term
        $ bundle update --ruby
        ```
        ");
    }
}
