//! ⁉️😄⁉️ - module purpose: what "canonical" means for the inventory files and who relies on it

use crate::base_image::DistroVersion;
use crate::{BaseImage, BuildTarget, RubyArtifact, RubyInventory, S3_BASE_URL};
use gem_version::GemVersion;
use libherokubuildpack::inventory::ParseInventoryError;
use libherokubuildpack::inventory::artifact::{Arch, Os};
use std::cmp::Reverse;
use std::collections::BTreeSet;
use std::fmt::{self, Display, Formatter};

/// ⁉️😄⁉️ - the uniqueness invariant this key enforces is not visible from the struct
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ArtifactKey {
    pub version: GemVersion,
    pub target: BuildTarget,
}

impl Display for ArtifactKey {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} on {} ({})",
            self.version, self.target.base_image, self.target.arch
        )
    }
}

#[derive(Debug, thiserror::Error)]
pub enum InventoryFormatError {
    #[error("Could not parse inventory: {0}")]
    Parse(#[source] ParseInventoryError),

    #[error("{version}: unsupported os {os}, expected linux")]
    UnsupportedOs { version: GemVersion, os: Os },

    #[error("{version}: distro_version {distro_version} is not a known base image")]
    UnknownDistroVersion {
        version: GemVersion,
        distro_version: DistroVersion,
    },

    #[error("{version}: {base_image} has no {arch} builds")]
    UnsupportedArch {
        version: GemVersion,
        base_image: BaseImage,
        arch: Arch,
    },

    #[error("{key}: url {url} is not under {S3_BASE_URL}")]
    UrlOutsideBucket { key: ArtifactKey, url: String },

    #[error("{0}: listed more than once")]
    Duplicate(ArtifactKey),

    #[error("Inventory is not in canonical form. Expected:\n{expected}")]
    NotCanonical { expected: String },
}

pub fn artifact_key(artifact: &RubyArtifact) -> Result<ArtifactKey, InventoryFormatError> {
    let version = artifact.version.clone();
    if artifact.os != Os::Linux {
        return Err(InventoryFormatError::UnsupportedOs {
            version,
            os: artifact.os,
        });
    }
    let distro_version = &artifact.metadata.distro_version;
    let base_image = BaseImage::from_distro_version(distro_version).ok_or_else(|| {
        InventoryFormatError::UnknownDistroVersion {
            version: version.clone(),
            distro_version: distro_version.clone(),
        }
    })?;
    if !base_image.archs().contains(&artifact.arch) {
        return Err(InventoryFormatError::UnsupportedArch {
            version,
            base_image,
            arch: artifact.arch,
        });
    }
    let key = ArtifactKey {
        version,
        target: BuildTarget {
            base_image,
            arch: artifact.arch,
        },
    };
    if !artifact.url.starts_with(&format!("{S3_BASE_URL}/")) {
        return Err(InventoryFormatError::UrlOutsideBucket {
            key,
            url: artifact.url.clone(),
        });
    }
    Ok(key)
}

/// ⁉️😄⁉️ - the sort order is part of the file format contract
pub fn sort_artifacts(inventory: &mut RubyInventory) -> Result<(), InventoryFormatError> {
    let mut keyed = inventory
        .artifacts
        .drain(..)
        .map(|artifact| artifact_key(&artifact).map(|key| (key, artifact)))
        .collect::<Result<Vec<_>, _>>()?;
    keyed.sort_by(|(a, _), (b, _)| {
        (Reverse(&a.version), &a.target).cmp(&(Reverse(&b.version), &b.target))
    });
    inventory.artifacts = keyed.into_iter().map(|(_, artifact)| artifact).collect();
    Ok(())
}

pub fn parse_inventory(contents: &str) -> Result<RubyInventory, InventoryFormatError> {
    let inventory = contents
        .parse::<RubyInventory>()
        .map_err(InventoryFormatError::Parse)?;
    let mut seen = BTreeSet::new();
    for artifact in &inventory.artifacts {
        let key = artifact_key(artifact)?;
        if !seen.insert(key.clone()) {
            return Err(InventoryFormatError::Duplicate(key));
        }
    }
    Ok(inventory)
}

pub fn render_inventory(mut inventory: RubyInventory) -> Result<String, InventoryFormatError> {
    sort_artifacts(&mut inventory)?;
    Ok(inventory.to_string())
}

pub fn check_inventory_format(contents: &str) -> Result<RubyInventory, InventoryFormatError> {
    let inventory = parse_inventory(contents)?;
    let expected = render_inventory(RubyInventory {
        artifacts: inventory.artifacts.clone(),
    })?;
    if expected == contents {
        Ok(inventory)
    } else {
        Err(InventoryFormatError::NotCanonical { expected })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ArtifactMetadata;
    use indoc::{formatdoc, indoc};
    use pretty_assertions::assert_eq;
    use std::str::FromStr;

    fn artifact(version: &str, base_image: &str, arch: Arch) -> RubyArtifact {
        RubyArtifact {
            version: GemVersion::from_str(version).unwrap(),
            os: Os::Linux,
            arch,
            url: format!("{S3_BASE_URL}/{base_image}/{arch}/ruby-{version}.tgz"),
            checksum: "sha256:dd073bda5665e758c3e6f861a6df435175c8e8faf5ec75bc2afaab1e3eebb2c7"
                .parse()
                .unwrap(),
            metadata: ArtifactMetadata {
                timestamp: "2026-04-22T16:44:16Z".parse().unwrap(),
                distro_version: BaseImage::new(base_image).unwrap().distro_version(),
            },
        }
    }

    fn inventory(artifacts: Vec<RubyArtifact>) -> RubyInventory {
        RubyInventory { artifacts }
    }

    #[test]
    fn empty_inventory_is_canonical() {
        check_inventory_format("artifacts = []\n").unwrap();
    }

    #[test]
    fn blank_file_is_rejected() {
        assert!(matches!(
            check_inventory_format("\n"),
            Err(InventoryFormatError::Parse(_))
        ));
    }

    #[test]
    fn render_sorts_newest_version_first_then_by_target() {
        let rendered = render_inventory(inventory(vec![
            artifact("3.4.1", "heroku-24", Arch::Arm64),
            artifact("3.4.10", "heroku-22", Arch::Amd64),
            artifact("3.4.1", "heroku-22", Arch::Amd64),
            artifact("3.4.1", "heroku-24", Arch::Amd64),
        ]))
        .unwrap();
        let order = parse_inventory(&rendered)
            .unwrap()
            .artifacts
            .iter()
            .map(|artifact| artifact_key(artifact).unwrap().to_string())
            .collect::<Vec<_>>();
        assert_eq!(
            order,
            vec![
                "3.4.10 on heroku-22 (amd64)",
                "3.4.1 on heroku-22 (amd64)",
                "3.4.1 on heroku-24 (amd64)",
                "3.4.1 on heroku-24 (arm64)",
            ]
        );
    }

    #[test]
    fn rendered_inventory_is_canonical() {
        let rendered = render_inventory(inventory(vec![
            artifact("3.3.0", "heroku-24", Arch::Amd64),
            artifact("3.4.0", "heroku-24", Arch::Amd64),
        ]))
        .unwrap();
        assert_eq!(
            rendered,
            formatdoc! {r#"
                [[artifacts]]
                version = "3.4.0"
                os = "linux"
                arch = "amd64"
                url = "{S3_BASE_URL}/heroku-24/amd64/ruby-3.4.0.tgz"
                checksum = "sha256:dd073bda5665e758c3e6f861a6df435175c8e8faf5ec75bc2afaab1e3eebb2c7"

                [artifacts.metadata]
                timestamp = "2026-04-22T16:44:16Z"
                distro_version = "24.04"

                [[artifacts]]
                version = "3.3.0"
                os = "linux"
                arch = "amd64"
                url = "{S3_BASE_URL}/heroku-24/amd64/ruby-3.3.0.tgz"
                checksum = "sha256:dd073bda5665e758c3e6f861a6df435175c8e8faf5ec75bc2afaab1e3eebb2c7"

                [artifacts.metadata]
                timestamp = "2026-04-22T16:44:16Z"
                distro_version = "24.04"
            "#}
        );
        check_inventory_format(&rendered).unwrap();
    }

    #[test]
    fn unsorted_inventory_is_not_canonical() {
        let sorted = render_inventory(inventory(vec![
            artifact("3.3.0", "heroku-24", Arch::Amd64),
            artifact("3.4.0", "heroku-24", Arch::Amd64),
        ]))
        .unwrap();
        let unsorted = inventory(vec![
            artifact("3.3.0", "heroku-24", Arch::Amd64),
            artifact("3.4.0", "heroku-24", Arch::Amd64),
        ])
        .to_string();
        match check_inventory_format(&unsorted) {
            Err(InventoryFormatError::NotCanonical { expected }) => assert_eq!(expected, sorted),
            other => panic!("expected NotCanonical, got {other:?}"),
        }
    }

    #[test]
    fn duplicate_artifact_is_rejected() {
        let contents = inventory(vec![
            artifact("3.4.0", "heroku-24", Arch::Amd64),
            artifact("3.4.0", "heroku-24", Arch::Amd64),
        ])
        .to_string();
        assert_eq!(
            check_inventory_format(&contents).unwrap_err().to_string(),
            "3.4.0 on heroku-24 (amd64): listed more than once"
        );
    }

    #[test]
    fn unknown_distro_version_is_rejected() {
        let contents = inventory(vec![artifact("3.4.0", "heroku-24", Arch::Amd64)])
            .to_string()
            .replace("24.04", "20.04");
        assert_eq!(
            check_inventory_format(&contents).unwrap_err().to_string(),
            "3.4.0: distro_version 20.04 is not a known base image"
        );
    }

    #[test]
    fn arch_not_built_for_base_image_is_rejected() {
        let mut arm = artifact("3.4.0", "heroku-22", Arch::Amd64);
        arm.arch = Arch::Arm64;
        let contents = inventory(vec![arm]).to_string();
        assert_eq!(
            check_inventory_format(&contents).unwrap_err().to_string(),
            "3.4.0: heroku-22 has no arm64 builds"
        );
    }

    #[test]
    fn darwin_is_rejected() {
        let mut darwin = artifact("3.4.0", "heroku-24", Arch::Amd64);
        darwin.os = Os::Darwin;
        let contents = inventory(vec![darwin]).to_string();
        assert_eq!(
            check_inventory_format(&contents).unwrap_err().to_string(),
            "3.4.0: unsupported os darwin, expected linux"
        );
    }

    #[test]
    fn url_outside_bucket_is_rejected() {
        let mut elsewhere = artifact("3.4.0", "heroku-24", Arch::Amd64);
        elsewhere.url = "https://example.com/ruby-3.4.0.tgz".to_owned();
        let contents = inventory(vec![elsewhere]).to_string();
        assert_eq!(
            check_inventory_format(&contents).unwrap_err().to_string(),
            format!(
                "3.4.0 on heroku-24 (amd64): url https://example.com/ruby-3.4.0.tgz is not under {S3_BASE_URL}"
            )
        );
    }

    #[test]
    fn invalid_toml_is_rejected() {
        let error = check_inventory_format(indoc! {r#"
            [[artifacts]]
            version = "3.4.0"
        "#})
        .unwrap_err();
        assert!(matches!(error, InventoryFormatError::Parse(_)));
    }
}
