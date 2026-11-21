use crate::base_image::{DistroVersion, UnknownDistroVersion};
use crate::{
    BaseImage, BuildTarget, Error, S3_BASE_URL, TarDownloadPath, UrlParseError, download_tar,
    parse_url,
};
use chrono::{DateTime, Utc};
use fs_err::{self as fs};
use gem_version::GemVersion;
use libherokubuildpack::inventory::Inventory;
use libherokubuildpack::inventory::artifact::{Arch, Artifact, Os};
use libherokubuildpack::inventory::checksum::Checksum;
use reqwest::Url;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::cmp::{Ordering, Reverse};
use std::fmt::{self, Display, Formatter};
use std::io::Read;
use std::ops::Deref;
use std::path::Path;

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct ArtifactMetadata {
    pub timestamp: DateTime<Utc>,
    pub distro_version: DistroVersion,
}

type InventoryArtifact = Artifact<GemVersion, Sha256, ArtifactMetadata>;

/// ⁉️😄⁉️ - what one key identifies, and that an inventory holds each key once
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactKey {
    pub version: GemVersion,
    pub target: BuildTarget,
}

/// ⁉️😄⁉️ - this is the inventory file order: newest version first, then by target
impl Ord for ArtifactKey {
    fn cmp(&self, other: &Self) -> Ordering {
        (Reverse(&self.version), &self.target).cmp(&(Reverse(&other.version), &other.target))
    }
}

impl PartialOrd for ArtifactKey {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
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

/// ⁉️😄⁉️ - what "valid" means, and why the fields are read only
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RubyArtifact {
    base_image: BaseImage,
    artifact: InventoryArtifact,
}

impl RubyArtifact {
    pub fn key(&self) -> ArtifactKey {
        ArtifactKey {
            version: self.artifact.version.clone(),
            target: BuildTarget {
                base_image: self.base_image.clone(),
                arch: self.artifact.arch,
            },
        }
    }
}

impl Deref for RubyArtifact {
    type Target = InventoryArtifact;

    fn deref(&self) -> &Self::Target {
        &self.artifact
    }
}

impl From<RubyArtifact> for InventoryArtifact {
    fn from(artifact: RubyArtifact) -> Self {
        artifact.artifact
    }
}

impl TryFrom<InventoryArtifact> for RubyArtifact {
    type Error = ArtifactError;

    fn try_from(artifact: InventoryArtifact) -> Result<Self, Self::Error> {
        let version = artifact.version.clone();
        if artifact.os != Os::Linux {
            return Err(ArtifactError::UnsupportedOs {
                version,
                os: artifact.os,
            });
        }
        let base_image =
            BaseImage::try_from(&artifact.metadata.distro_version).map_err(|source| {
                ArtifactError::UnknownDistroVersion {
                    version: version.clone(),
                    source,
                }
            })?;
        if !base_image.archs().contains(&artifact.arch) {
            return Err(ArtifactError::UnsupportedArch {
                version,
                base_image,
                arch: artifact.arch,
            });
        }
        let ruby = Self {
            base_image,
            artifact,
        };
        let url = parse_url(&ruby.url).map_err(|source| ArtifactError::InvalidUrl {
            key: ruby.key(),
            source,
        })?;
        let bucket = parse_url(S3_BASE_URL).expect("internal constant should be a valid URL");
        if url.origin() != bucket.origin() {
            return Err(ArtifactError::UrlOutsideBucket {
                key: ruby.key(),
                url: Box::new(url),
            });
        }
        Ok(ruby)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ArtifactError {
    #[error("{version}: unsupported os {os}, expected linux")]
    UnsupportedOs { version: GemVersion, os: Os },

    #[error("{version}: {source}")]
    UnknownDistroVersion {
        version: GemVersion,
        source: UnknownDistroVersion,
    },

    #[error("{version}: {base_image} has no {arch} builds")]
    UnsupportedArch {
        version: GemVersion,
        base_image: BaseImage,
        arch: Arch,
    },

    #[error("{key}: {source}")]
    InvalidUrl {
        key: ArtifactKey,
        source: UrlParseError,
    },

    #[error("{key}: url {url} is not under {S3_BASE_URL}")]
    UrlOutsideBucket { key: ArtifactKey, url: Box<Url> },
}

/// ```no_run
/// use shared::inventory_check;
///
/// let contents = r#"
/// [[artifacts]]
/// version = "9.4.8.0"
/// os = "linux"
/// arch = "amd64"
/// url = "https://heroku-buildpack-ruby.s3.dualstack.us-east-1.amazonaws.com/heroku-24/ruby-3.1.4-jruby-9.4.8.0.tgz"
/// checksum = "sha256:815b31d2b204a524bf74aabae341bf85353add4d1128d5d276d08fa5e8ff3c39"
///
/// [artifacts.metadata]
/// timestamp = "2024-07-24T16:17:35.341413Z"
/// distro_version = "24.04"
/// "#;
/// tokio::runtime::Runtime::new().unwrap().block_on(inventory_check(contents)).unwrap();
/// ```
pub async fn inventory_check(contents: &str) -> Result<(), Error> {
    if contents.trim().is_empty() {
        return Ok(());
    }

    let inventory = contents
        .parse::<Inventory<GemVersion, Sha256, ArtifactMetadata>>()
        .map_err(|e| Error::Other(format!("Could not parse inventory. Error: {e}")))?;

    let mut set = tokio::task::JoinSet::new();
    for artifact in inventory.artifacts {
        set.spawn(async move {
            let temp = tempfile::tempdir().map_err(|e| format!("Error {e}"))?;
            let path = temp.path().join("file.tar");

            download_tar(&artifact.url, &TarDownloadPath(path.clone()))
                .await
                .map_err(|e| format!("Error {e}"))?;

            // Computing a file Hash is CPU bound <https://docs.rs/tokio/latest/tokio/index.html#cpu-bound-tasks-and-blocking-code>
            let checksum_string: String = tokio::task::spawn_blocking(move || {
                sha256_from_path(&path).map_err(|e| format!("Error {e}"))
            })
            .await
            .map_err(|e| format!("Error {e}"))??;

            let checksum: Checksum<Sha256> = format!("sha256:{checksum_string}")
                .parse()
                .map_err(|e| format!("Error {e}"))?;

            if checksum == artifact.checksum {
                Ok::<(), String>(())
            } else {
                Err(format!(
                    "Checksum mismatch for {url} expected {expected} got {actual}",
                    url = artifact.url,
                    expected = hex::encode(&artifact.checksum.value),
                    actual = hex::encode(&checksum.value)
                ))
            }
        });
    }

    let mut errors = Vec::new();
    while let Some(joined) = set.join_next().await {
        match joined {
            Ok(Ok(())) => {}
            Ok(Err(message)) => errors.push(message),
            Err(join_error) => errors.push(format!("Error {join_error}")),
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(Error::Other(errors.join("\n")))
    }
}

/// Returns the sha256 hash of the file at the given path
pub fn sha256_from_path(path: &Path) -> Result<String, Error> {
    digest::<Sha256>(fs::File::open(path).map_err(Error::FsError)?)
        .map(hex::encode)
        .map_err(|e| {
            Error::Other(format!(
                "Error {e} calculating sha256 for {path}",
                path = path.display()
            ))
        })
}

pub fn digest<D>(mut input: impl Read) -> Result<sha2::digest::Output<D>, std::io::Error>
where
    D: Default + sha2::digest::Update + sha2::digest::FixedOutput,
{
    let mut digest = D::default();

    let mut buffer = [0x00; 1024];
    loop {
        let bytes_read = input.read(&mut buffer)?;

        if bytes_read > 0 {
            digest.update(&buffer[..bytes_read]);
        } else {
            break;
        }
    }

    Ok(digest.finalize_fixed())
}

#[cfg(test)]
mod test {
    use super::*;

    fn artifact(version: &str, base_image: &str, arch: Arch) -> InventoryArtifact {
        InventoryArtifact {
            version: version.parse().unwrap(),
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

    #[test]
    fn rejects_arch_not_built_for_base_image() {
        let mut arm = artifact("3.4.0", "heroku-22", Arch::Amd64);
        arm.arch = Arch::Arm64;
        let error = RubyArtifact::try_from(arm).unwrap_err();
        assert_eq!(error.to_string(), "3.4.0: heroku-22 has no arm64 builds");
    }

    #[test]
    fn rejects_darwin() {
        let mut darwin = artifact("3.4.0", "heroku-24", Arch::Amd64);
        darwin.os = Os::Darwin;
        let error = RubyArtifact::try_from(darwin).unwrap_err();
        assert_eq!(
            error.to_string(),
            "3.4.0: unsupported os darwin, expected linux"
        );
    }

    #[test]
    fn rejects_url_outside_bucket() {
        let mut elsewhere = artifact("3.4.0", "heroku-24", Arch::Amd64);
        elsewhere.url = "https://example.com/ruby-3.4.0.tgz".to_owned();
        let error = RubyArtifact::try_from(elsewhere).unwrap_err();
        assert_eq!(
            error.to_string(),
            format!(
                "3.4.0 on heroku-24 (amd64): url https://example.com/ruby-3.4.0.tgz is not under {S3_BASE_URL}"
            )
        );
    }

    #[test]
    fn rejects_url_that_does_not_parse() {
        let mut broken = artifact("3.4.0", "heroku-24", Arch::Amd64);
        broken.url = "not a url".to_owned();
        let error = RubyArtifact::try_from(broken).unwrap_err();
        assert!(matches!(error, ArtifactError::InvalidUrl { .. }));
    }
}
