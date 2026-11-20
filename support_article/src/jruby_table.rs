//! One row per JRuby `MAJOR.MINOR` line, newest first, with the newest release available on each base image.

use crate::{CheckError, complete_base_images};
use jruby_executable::JRubyVersion;
use minijinja::{Value, context};
use shared::{BaseImage, MajorMinor, RubyArtifact, RubyDownloadVersion, RubyInventory, parse_url};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::{self, Display, Formatter};
use std::str::FromStr;

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

/// Every release in `inventory` at or above `floor`.
pub fn builds(inventory: &RubyInventory, floor: MajorMinor) -> Result<Vec<JRubyBuild>, CheckError> {
    let mut ruby_compat = BTreeMap::new();
    for artifact in inventory.artifacts() {
        ruby_compat
            .entry(artifact.version.clone())
            .or_insert(ruby_compat_from_url(artifact)?);
    }
    let mut builds = Vec::new();
    for (version, base_images) in complete_base_images(inventory) {
        let jruby_version = JRubyVersion::from_str(&version.to_string())?;
        if MajorMinor::from(&jruby_version) >= floor {
            builds.push(JRubyBuild {
                release: JRubyRelease {
                    version: jruby_version,
                    ruby_compat: ruby_compat[&version].clone(),
                },
                base_images,
            });
        }
    }
    Ok(builds)
}

/// ⁉️😄⁉️ - why the stdlib version comes from the file name, and who writes that name
fn ruby_compat_from_url(artifact: &RubyArtifact) -> Result<RubyDownloadVersion, CheckError> {
    let url = parse_url(&artifact.url)?;
    let stdlib = url
        .path_segments()
        .and_then(|mut segments| segments.next_back())
        .and_then(|name| name.strip_prefix("ruby-"))
        .and_then(|rest| rest.split_once("-jruby-"))
        .map(|(stdlib, _)| stdlib.to_owned())
        .ok_or_else(|| CheckError::NotAJRubyUrl(url.clone()))?;
    Ok(RubyDownloadVersion::new(stdlib)?)
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
    use libherokubuildpack::inventory::artifact::Arch;
    use pretty_assertions::assert_eq;
    use shared::S3_BASE_URL;

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

    #[test]
    fn builds_reads_the_stdlib_version_from_the_inventory_url() {
        let mut artifacts = crate::tests::artifacts(&[
            ("10.0.2.0", "heroku-24", Arch::Amd64),
            ("10.0.2.0", "heroku-24", Arch::Arm64),
            ("9.3.15.0", "heroku-24", Arch::Amd64),
            ("9.3.15.0", "heroku-24", Arch::Arm64),
        ]);
        for artifact in &mut artifacts {
            artifact.url = format!(
                "{S3_BASE_URL}/heroku-24/ruby-3.4.5-jruby-{}-abcdef0.tgz",
                artifact.version
            );
        }
        let inventory = crate::tests::sorted(artifacts);
        assert_eq!(
            builds(&inventory, "9.4".parse().unwrap()).unwrap(),
            vec![with_ruby_compat("3.4.5", build("10.0.2.0", &["heroku-24"]))]
        );
    }

    #[test]
    fn builds_rejects_a_url_without_a_stdlib_version() {
        let inventory = crate::tests::inventory(&[("10.0.2.0", "heroku-24", Arch::Amd64)]);
        assert!(matches!(
            builds(&inventory, "9.4".parse().unwrap()),
            Err(CheckError::NotAJRubyUrl(_))
        ));
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
