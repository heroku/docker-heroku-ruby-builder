//! One row per Ruby `MAJOR.MINOR` line, newest first, with the newest release available on each base image.

use crate::{CheckError, join_all};
use minijinja::{Value, context};
use ruby_executable::ruby_lang::BranchStatus;
use shared::{
    BaseImage, BuildTarget, MajorMinor, RubyDownloadVersion, output_ruby_tar_path, s3_presence,
    s3_url,
};
use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use tokio::task::JoinSet;
use url::Url;

/// Upstream maintenance status of a Ruby line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lifecycle {
    Maintained,
    SecurityOnly,
    Eol,
}

/// A Ruby release and the base images it is built for on every architecture.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RubyBuild {
    pub version: RubyDownloadVersion,
    pub base_images: BTreeSet<BaseImage>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RubyRow {
    pub major_minor: MajorMinor,
    pub lifecycle: Lifecycle,
    /// The newest release available on each base image.
    pub versions: BTreeMap<BaseImage, RubyDownloadVersion>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RubyTable {
    pub rows: Vec<RubyRow>,
    /// The version shown in the Gemfile example: the newest one on the newest base image.
    pub example: RubyDownloadVersion,
}

/// An empty table means an upstream source or S3 lookup is broken, so it is not published.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("no Ruby release at or above {floor} is available on any base image")]
pub struct EmptyTable {
    pub floor: MajorMinor,
}

/// Where `version` lives on S3 for `target`.
///
/// `heroku-22` is checked at its legacy path without an architecture directory. Older `heroku-22` builds exist only
/// there.
fn tar_url(version: &RubyDownloadVersion, target: &BuildTarget) -> Url {
    let base_image = &target.base_image;
    let arch_dir = (!base_image.has_legacy_path()).then_some(&target.arch);
    s3_url(&output_ruby_tar_path(
        Path::new(""),
        version,
        base_image,
        arch_dir,
    ))
}

/// Checks S3 for every stable release at or above `floor`.
pub async fn builds(
    releases: &[RubyDownloadVersion],
    floor: MajorMinor,
) -> Result<Vec<RubyBuild>, CheckError> {
    let mut set = JoinSet::new();
    for version in releases
        .iter()
        .filter(|version| version.is_prerelease().is_none() && MajorMinor::from(*version) >= floor)
        .cloned()
    {
        set.spawn(async move {
            let presence = s3_presence(|target| tar_url(&version, target)).await?;
            Ok(RubyBuild {
                version,
                base_images: presence.complete_base_images().into_iter().collect(),
            })
        });
    }
    join_all(set).await
}

impl RubyTable {
    /// A row for every non-preview branch at or above `floor`, built from the stable releases in `builds`. A line with
    /// nothing available is left out.
    pub fn new(
        branches: &[(MajorMinor, BranchStatus)],
        builds: &[RubyBuild],
        floor: MajorMinor,
    ) -> Result<RubyTable, EmptyTable> {
        let mut rows = branches
            .iter()
            .filter_map(|&(major_minor, status)| {
                let lifecycle = match status {
                    BranchStatus::NormalMaintenance => Lifecycle::Maintained,
                    BranchStatus::SecurityMaintenance => Lifecycle::SecurityOnly,
                    BranchStatus::Eol => Lifecycle::Eol,
                    BranchStatus::Preview => return None,
                };
                if major_minor < floor {
                    return None;
                }
                let mut versions = BTreeMap::<BaseImage, RubyDownloadVersion>::new();
                for build in builds.iter().filter(|build| {
                    MajorMinor::from(&build.version) == major_minor
                        && build.version.is_prerelease().is_none()
                }) {
                    for base_image in &build.base_images {
                        if versions
                            .get(base_image)
                            .is_none_or(|newest| build.version > *newest)
                        {
                            versions.insert(base_image.clone(), build.version.clone());
                        }
                    }
                }
                (!versions.is_empty()).then_some(RubyRow {
                    major_minor,
                    lifecycle,
                    versions,
                })
            })
            .collect::<Vec<_>>();
        rows.sort_by_key(|row| Reverse(row.major_minor));
        let (_, example) = rows
            .iter()
            .flat_map(|row| &row.versions)
            .max()
            .ok_or(EmptyTable { floor })?;
        Ok(RubyTable {
            example: example.clone(),
            rows,
        })
    }

    /// The table's columns.
    pub fn base_images(&self) -> BTreeSet<&BaseImage> {
        self.rows
            .iter()
            .flat_map(|row| row.versions.keys())
            .collect()
    }
}

impl RubyTable {
    /// Markdown with `\n` line endings: the HTML table, then a Gemfile example.
    pub fn render(&self) -> Result<String, minijinja::Error> {
        crate::render_template(
            "ruby_versions_table.md.jinja",
            include_str!("../templates/ruby_versions_table.md.jinja"),
            context! {
                base_images => self.base_images().iter().map(ToString::to_string).collect::<Value>(),
                rows => self.render_row_values(),
                example => self.example.to_string(),
            },
        )
    }

    fn render_row_values(&self) -> Value {
        self.rows
            .iter()
            .map(|row| {
                let (style, label) = match row.lifecycle {
                    Lifecycle::Maintained => (None, ""),
                    Lifecycle::SecurityOnly => (
                        Some("background-color:#fcf8e3;color:#8a6d3b;"),
                        " (security-only)",
                    ),
                    Lifecycle::Eol => (Some("background-color:#f2dede;color:#a94442;"), " (EOL)"),
                };
                context! {
                    major_minor => row.major_minor.to_string(),
                    style,
                    label,
                    versions => self.base_images()
                        .iter()
                        .map(|base_image| row.versions.get(base_image).map(ToString::to_string))
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
    use shared::build_matrix;

    fn branch(name: &str, status: BranchStatus) -> (MajorMinor, BranchStatus) {
        (name.parse().unwrap(), status)
    }

    fn version(s: &str) -> RubyDownloadVersion {
        RubyDownloadVersion::new(s).unwrap()
    }

    fn build(name: &str, base_images: &[&str]) -> RubyBuild {
        RubyBuild {
            version: version(name),
            base_images: base_images
                .iter()
                .map(|name| BaseImage::new(name).unwrap())
                .collect(),
        }
    }

    fn summary(table: &RubyTable) -> Vec<String> {
        table
            .rows
            .iter()
            .map(|row| {
                let cells = table
                    .base_images()
                    .iter()
                    .map(|base_image| {
                        row.versions
                            .get(base_image)
                            .map_or("-".to_string(), ToString::to_string)
                    })
                    .collect::<Vec<_>>()
                    .join(" ");
                format!("{} {:?}: {cells}", row.major_minor, row.lifecycle)
            })
            .collect()
    }

    #[test]
    fn checks_heroku_22_at_the_legacy_path_and_other_base_images_per_arch() {
        let paths = build_matrix()
            .iter()
            .map(|target| tar_url(&version("8.1.2"), target).path().to_string())
            .collect::<Vec<_>>();

        for expected in [
            "/heroku-22/ruby-8.1.2.tgz",
            "/heroku-24/amd64/ruby-8.1.2.tgz",
            "/heroku-24/arm64/ruby-8.1.2.tgz",
        ] {
            assert!(paths.contains(&expected.to_string()), "{paths:?}");
        }
        assert!(
            !paths
                .iter()
                .any(|path| path.starts_with("/heroku-22/amd64")),
            "{paths:?}"
        );
    }

    #[test]
    fn shows_the_newest_stable_release_available_on_each_base_image() {
        let branches = [
            branch("8.3", BranchStatus::Preview),
            branch("8.1", BranchStatus::SecurityMaintenance),
            branch("8.2", BranchStatus::NormalMaintenance),
            branch("8.0", BranchStatus::Eol),
            branch("7.9", BranchStatus::Eol),
        ];
        let builds = [
            build("8.3.0-preview1", &["heroku-24", "heroku-26"]),
            build("8.2.3-rc1", &["heroku-24", "heroku-26"]),
            build("8.2.2", &["heroku-24"]),
            build("8.2.1", &["heroku-24", "heroku-26"]),
            build("8.1.9", &["heroku-24", "heroku-26"]),
            build("8.1.8", &["heroku-24", "heroku-26"]),
            build("8.0.7", &["heroku-24"]),
            build("7.9.5", &["heroku-24", "heroku-26"]),
        ];

        let table = RubyTable::new(&branches, &builds, "8.0".parse().unwrap()).unwrap();

        insta::assert_snapshot!(summary(&table).join("\n"), @r"
        8.2 Maintained: 8.2.2 8.2.1
        8.1 SecurityOnly: 8.1.9 8.1.9
        8.0 Eol: 8.0.7 -
        ");
    }

    #[test]
    fn leaves_out_a_line_with_nothing_available() {
        let branches = [
            branch("8.2", BranchStatus::NormalMaintenance),
            branch("8.1", BranchStatus::SecurityMaintenance),
        ];
        let builds = [build("8.2.0", &[]), build("8.1.9", &["heroku-24"])];

        let table = RubyTable::new(&branches, &builds, "8.0".parse().unwrap()).unwrap();

        assert_eq!(summary(&table), vec!["8.1 SecurityOnly: 8.1.9"]);
    }

    #[test]
    fn errors_when_nothing_is_available() {
        let branches = [branch("8.2", BranchStatus::NormalMaintenance)];
        let builds = [build("8.2.0", &[])];

        let error = RubyTable::new(&branches, &builds, "8.0".parse().unwrap()).unwrap_err();

        assert_eq!(
            error.to_string(),
            "no Ruby release at or above 8.0 is available on any base image"
        );
    }

    #[test]
    fn leaves_out_a_base_image_with_nothing_available() {
        let branches = [branch("8.2", BranchStatus::NormalMaintenance)];
        let builds = [build("8.2.1", &["heroku-24"])];

        let table = RubyTable::new(&branches, &builds, "8.0".parse().unwrap()).unwrap();

        assert_eq!(summary(&table), vec!["8.2 Maintained: 8.2.1"]);
    }

    fn rendered_fixture() -> RubyTable {
        let branches = [
            branch("8.2", BranchStatus::NormalMaintenance),
            branch("8.1", BranchStatus::SecurityMaintenance),
            branch("8.0", BranchStatus::Eol),
        ];
        let builds = [
            build("8.2.1", &["heroku-24", "heroku-26"]),
            build("8.1.9", &["heroku-24", "heroku-26"]),
            build("8.0.7", &["heroku-24"]),
        ];
        RubyTable::new(&branches, &builds, "8.0".parse().unwrap()).unwrap()
    }

    #[test]
    fn example_is_the_newest_release_on_the_newest_base_image() {
        let branches = [
            branch("8.2", BranchStatus::NormalMaintenance),
            branch("8.1", BranchStatus::SecurityMaintenance),
        ];
        let builds = [
            build("8.2.2", &["heroku-24"]),
            build("8.2.1", &["heroku-24", "heroku-26"]),
            build("8.1.9", &["heroku-26"]),
        ];

        let table = RubyTable::new(&branches, &builds, "8.0".parse().unwrap()).unwrap();

        assert_eq!(table.example, version("8.2.1"));
    }

    #[test]
    fn renders_one_tag_per_line_with_lifecycle_labels_and_styles() {
        insta::assert_snapshot!(rendered_fixture().render().unwrap(), @r#"
        <table>
          <thead>
            <tr>
              <th>Ruby series</th>
                <th>heroku-24</th>
                <th>heroku-26</th>
            </tr>
          </thead>
          <tbody>
              <tr>
                <td>8.2</td>
                  <td>8.2.1</td>
                  <td>8.2.1</td>
              </tr>
              <tr style="background-color:#fcf8e3;color:#8a6d3b;">
                <td>8.1 (security-only)</td>
                  <td>8.1.9</td>
                  <td>8.1.9</td>
              </tr>
              <tr style="background-color:#f2dede;color:#a94442;">
                <td>8.0 (EOL)</td>
                  <td>8.0.7</td>
                  <td>-</td>
              </tr>
          </tbody>
        </table>

        To use a Ruby version, specify it in your `Gemfile`. For example:

        ```ruby
        # Gemfile
        ruby '8.2.1'
        ```

        Then, with that Ruby version installed locally, update your `Gemfile.lock` by running:

        ```term
        $ bundle update --ruby
        ```
        "#);
    }
}
