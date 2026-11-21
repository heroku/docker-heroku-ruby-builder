use crate::base_image::{DistroVersion, UnknownDistroVersion};
use crate::{
    BaseImage, BuildTarget, Error, S3_BASE_URL, TarDownloadPath, UrlParseError, download_tar,
    parse_url,
};
use chrono::{DateTime, Utc};
use fs_err::{self as fs};
use gem_version::GemVersion;
use libherokubuildpack::inventory::artifact::{Arch, Artifact, Os};
use libherokubuildpack::inventory::checksum::Checksum;
use libherokubuildpack::inventory::{Inventory, ParseInventoryError};
use reqwest::Url;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::cmp::{Ordering, Reverse};
use std::collections::BTreeMap;
use std::fmt::{self, Display, Formatter};
use std::io::Read;
use std::ops::Deref;
use std::path::Path;
use std::str::FromStr;

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct ArtifactMetadata {
    pub timestamp: DateTime<Utc>,
    pub distro_version: DistroVersion,
}

type InventoryFile = Inventory<GemVersion, Sha256, ArtifactMetadata>;
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

/// ⁉️😄⁉️ - why the order matters: the sync automation must write the same bytes for the same builds
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RubyInventory(BTreeMap<ArtifactKey, RubyArtifact>);

impl RubyInventory {
    pub fn new(artifacts: Vec<RubyArtifact>) -> Result<Self, InventoryError> {
        let mut map = BTreeMap::new();
        for artifact in artifacts {
            let key = artifact.key();
            if map.insert(key.clone(), artifact).is_some() {
                return Err(InventoryError::Duplicate(key));
            }
        }
        Ok(Self(map))
    }

    pub fn artifacts(&self) -> impl ExactSizeIterator<Item = &RubyArtifact> {
        self.0.values()
    }
}

impl TryFrom<InventoryFile> for RubyInventory {
    type Error = InventoryError;

    fn try_from(inventory: InventoryFile) -> Result<Self, Self::Error> {
        Self::new(
            inventory
                .artifacts
                .into_iter()
                .map(RubyArtifact::try_from)
                .collect::<Result<_, _>>()?,
        )
    }
}

impl FromStr for RubyInventory {
    type Err = InventoryError;

    fn from_str(contents: &str) -> Result<Self, Self::Err> {
        Self::try_from(
            contents
                .parse::<InventoryFile>()
                .map_err(InventoryError::Parse)?,
        )
    }
}

impl Display for RubyInventory {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        InventoryFile {
            artifacts: self.0.values().cloned().map(Into::into).collect(),
        }
        .fmt(f)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum InventoryError {
    #[error("Could not parse inventory: {0}")]
    Parse(#[source] ParseInventoryError),

    #[error(transparent)]
    Artifact(#[from] ArtifactError),

    #[error("{0}: listed more than once")]
    Duplicate(ArtifactKey),
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

    fn inventory(artifacts: Vec<InventoryArtifact>) -> RubyInventory {
        RubyInventory::try_from(InventoryFile { artifacts }).unwrap()
    }

    fn keys(inventory: &RubyInventory) -> Vec<String> {
        inventory
            .artifacts()
            .map(|artifact| artifact.key().to_string())
            .collect()
    }

    fn unsorted_toml(artifacts: Vec<InventoryArtifact>) -> String {
        InventoryFile { artifacts }.to_string()
    }

    #[test]
    fn sorts_newest_version_first_then_by_target() {
        let inventory = inventory(vec![
            artifact("3.4.1", "heroku-24", Arch::Arm64),
            artifact("3.4.10", "heroku-22", Arch::Amd64),
            artifact("3.4.1", "heroku-22", Arch::Amd64),
            artifact("3.4.1", "heroku-24", Arch::Amd64),
        ]);
        assert_eq!(
            keys(&inventory),
            vec![
                "3.4.10 on heroku-22 (amd64)",
                "3.4.1 on heroku-22 (amd64)",
                "3.4.1 on heroku-24 (amd64)",
                "3.4.1 on heroku-24 (arm64)",
            ]
        );
    }

    #[test]
    fn parses_what_it_displays() {
        let inventory = inventory(vec![
            artifact("3.3.0", "heroku-24", Arch::Amd64),
            artifact("3.4.0", "heroku-24", Arch::Amd64),
        ]);
        assert_eq!(
            inventory.to_string().parse::<RubyInventory>().unwrap(),
            inventory
        );
    }

    #[test]
    fn empty_inventory_parses() {
        "artifacts = []\n".parse::<RubyInventory>().unwrap();
    }

    #[test]
    fn parse_sorts_artifacts() {
        let contents = unsorted_toml(vec![
            artifact("3.3.0", "heroku-24", Arch::Amd64),
            artifact("3.4.0", "heroku-24", Arch::Amd64),
        ]);
        assert_eq!(
            keys(&contents.parse::<RubyInventory>().unwrap()),
            vec!["3.4.0 on heroku-24 (amd64)", "3.3.0 on heroku-24 (amd64)"]
        );
    }

    #[test]
    fn rejects_duplicate_artifacts() {
        let error = RubyInventory::try_from(InventoryFile {
            artifacts: vec![
                artifact("3.4.0", "heroku-24", Arch::Amd64),
                artifact("3.4.0", "heroku-24", Arch::Amd64),
            ],
        })
        .unwrap_err();
        assert_eq!(
            error.to_string(),
            "3.4.0 on heroku-24 (amd64): listed more than once"
        );
    }

    #[test]
    fn rejects_unknown_distro_version() {
        let contents = unsorted_toml(vec![artifact("3.4.0", "heroku-24", Arch::Amd64)])
            .replace("24.04", "20.04");
        let error = contents.parse::<RubyInventory>().unwrap_err();
        assert_eq!(
            error.to_string(),
            "3.4.0: distro_version 20.04 is not a known base image"
        );
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

    #[test]
    fn rejects_invalid_toml() {
        let error = "[[artifacts]]\nversion = \"3.4.0\"\n"
            .parse::<RubyInventory>()
            .unwrap_err();
        assert!(matches!(error, InventoryError::Parse(_)));
    }

    #[test]
    fn repo_inventory_files_parse() {
        for (file, contents) in [
            (
                "ruby_inventory.toml",
                include_str!("../../ruby_inventory.toml"),
            ),
            (
                "jruby_inventory.toml",
                include_str!("../../jruby_inventory.toml"),
            ),
        ] {
            contents
                .parse::<RubyInventory>()
                .unwrap_or_else(|error| panic!("{file}: {error}"));
        }
    }
}
