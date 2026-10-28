//! One row per Ruby `MAJOR.MINOR` line, newest first, with the newest release available on each base image.

use ruby_executable::ruby_lang::BranchStatus;
use shared::{BaseImage, MajorMinor, RubyDownloadVersion};
use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet};

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
}

impl RubyTable {
    /// A row for every non-preview branch at or above `floor`, built from the stable releases in `builds`. A line with
    /// nothing available is left out.
    pub fn new(
        branches: &[(MajorMinor, BranchStatus)],
        builds: &[RubyBuild],
        floor: MajorMinor,
    ) -> RubyTable {
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
        RubyTable { rows }
    }

    /// The table's columns.
    pub fn base_images(&self) -> BTreeSet<&BaseImage> {
        self.rows
            .iter()
            .flat_map(|row| row.versions.keys())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

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

        let table = RubyTable::new(&branches, &builds, "8.0".parse().unwrap());

        insta::assert_snapshot!(summary(&table).join("\n"), @r"
        8.2 Maintained: 8.2.2 8.2.1
        8.1 SecurityOnly: 8.1.9 8.1.9
        8.0 Eol: 8.0.7 -
        ");
    }

    #[test]
    fn leaves_out_a_line_with_nothing_available() {
        let branches = [branch("8.2", BranchStatus::NormalMaintenance)];
        let builds = [build("8.2.0", &[])];

        let table = RubyTable::new(&branches, &builds, "8.0".parse().unwrap());

        assert_eq!(summary(&table), Vec::<String>::new());
    }

    #[test]
    fn leaves_out_a_base_image_with_nothing_available() {
        let branches = [branch("8.2", BranchStatus::NormalMaintenance)];
        let builds = [build("8.2.1", &["heroku-24"])];

        let table = RubyTable::new(&branches, &builds, "8.0".parse().unwrap());

        assert_eq!(summary(&table), vec!["8.2 Maintained: 8.2.1"]);
    }
}
