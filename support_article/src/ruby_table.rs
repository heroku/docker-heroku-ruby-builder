//! One row per Ruby `MAJOR.MINOR` line, newest first, with the newest release available on each base image.

use crate::{CheckError, MajorMinor, UNAVAILABLE, join_all, render};
use libherokubuildpack::inventory::artifact::Arch;
use minijinja::{Value, context};
use ruby_executable::ruby_lang::{BranchStatus, RubyBranch};
use shared::{
    BaseImage, RubyDownloadVersion, build_matrix, output_ruby_tar_path, s3_presence, s3_url,
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
    pub base_images: Vec<BaseImage>,
    pub rows: Vec<RubyRow>,
}

/// Where `version` lives on S3 for each base image and architecture.
///
/// `heroku-22` is checked at its legacy path without an architecture directory. Older `heroku-22` builds exist only
/// there.
fn tar_urls(version: &RubyDownloadVersion) -> Vec<(Url, BaseImage, Arch)> {
    build_matrix()
        .into_iter()
        .map(|(base_image, arch)| {
            let arch_dir = (!base_image.has_legacy_path()).then_some(&arch);
            let path = output_ruby_tar_path(Path::new(""), version, &base_image, arch_dir);
            (s3_url(&path), base_image, arch)
        })
        .collect()
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
            let presence = s3_presence(tar_urls(&version)).await?;
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
        branches: &[RubyBranch],
        builds: &[RubyBuild],
        floor: MajorMinor,
        base_images: Vec<BaseImage>,
    ) -> RubyTable {
        let mut rows = branches
            .iter()
            .filter_map(|branch| {
                let lifecycle = match branch.status {
                    BranchStatus::NormalMaintenance => Lifecycle::Maintained,
                    BranchStatus::SecurityMaintenance => Lifecycle::SecurityOnly,
                    BranchStatus::Eol => Lifecycle::Eol,
                    BranchStatus::Preview => return None,
                };
                let major_minor = MajorMinor::from(branch);
                if major_minor < floor {
                    return None;
                }
                let versions = base_images
                    .iter()
                    .filter_map(|base_image| {
                        builds
                            .iter()
                            .filter(|build| {
                                MajorMinor::from(&build.version) == major_minor
                                    && build.version.is_prerelease().is_none()
                                    && build.base_images.contains(base_image)
                            })
                            .map(|build| build.version.clone())
                            .max()
                            .map(|version| (base_image.clone(), version))
                    })
                    .collect::<BTreeMap<_, _>>();
                (!versions.is_empty()).then_some(RubyRow {
                    major_minor,
                    lifecycle,
                    versions,
                })
            })
            .collect::<Vec<_>>();
        rows.sort_by_key(|row| Reverse(row.major_minor));
        RubyTable { base_images, rows }
    }
}

impl RubyTable {
    /// HTML with `\n` line endings.
    pub fn render(&self) -> String {
        let rows = self
            .rows
            .iter()
            .map(|row| {
                let lifecycle = match row.lifecycle {
                    Lifecycle::Maintained => "maintained",
                    Lifecycle::SecurityOnly => "security_only",
                    Lifecycle::Eol => "eol",
                };
                context! {
                    major_minor => row.major_minor.to_string(),
                    lifecycle,
                    versions => self
                        .base_images
                        .iter()
                        .map(|base_image| row.versions.get(base_image).map(ToString::to_string))
                        .collect::<Value>(),
                }
            })
            .collect::<Value>();
        render(
            "ruby_versions_table.html",
            include_str!("../templates/ruby_versions_table.html"),
            context! {
                base_images => self.base_images.iter().map(ToString::to_string).collect::<Value>(),
                rows,
                unavailable => UNAVAILABLE,
            },
        )
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

    fn branch(name: &str, status: BranchStatus) -> RubyBranch {
        let major_minor: MajorMinor = name.parse().unwrap();
        RubyBranch {
            major: major_minor.major,
            minor: major_minor.minor,
            status,
        }
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
                    .base_images
                    .iter()
                    .map(|base_image| {
                        row.versions
                            .get(base_image)
                            .map_or(UNAVAILABLE.to_string(), ToString::to_string)
                    })
                    .collect::<Vec<_>>()
                    .join(" ");
                format!("{} {:?}: {cells}", row.major_minor, row.lifecycle)
            })
            .collect()
    }

    #[test]
    fn checks_heroku_22_at_the_legacy_path_and_other_base_images_per_arch() {
        let paths = tar_urls(&version("8.1.2"))
            .into_iter()
            .map(|(url, _, _)| url.path().to_string())
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

        let table = RubyTable::new(&branches, &builds, "8.0".parse().unwrap(), base_images());

        assert_eq!(
            summary(&table),
            vec![
                "8.2 Maintained: 8.2.2 8.2.1",
                "8.1 SecurityOnly: 8.1.9 8.1.9",
                "8.0 Eol: 8.0.7 –",
            ]
        );
    }

    #[test]
    fn leaves_out_a_line_with_nothing_available() {
        let branches = [branch("8.2", BranchStatus::NormalMaintenance)];
        let builds = [build("8.2.0", &[])];

        let table = RubyTable::new(&branches, &builds, "8.0".parse().unwrap(), base_images());

        assert_eq!(summary(&table), Vec::<String>::new());
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
        RubyTable::new(&branches, &builds, "8.0".parse().unwrap(), base_images())
    }

    #[test]
    fn renders_one_tag_per_line_with_lifecycle_labels_and_styles() {
        assert_eq!(
            rendered_fixture().render(),
            indoc! {r#"
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
                <td>–</td>
                </tr>
                </tbody>
                </table>"#}
        );
    }
}
