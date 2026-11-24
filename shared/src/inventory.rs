use crate::base_image::{DistroVersion, UnknownDistroVersion};
use crate::{
    BaseImage, BuildTarget, Error, NonEmpty, S3_BASE_URL, TarDownloadPath, UrlParseError,
    download_tar, parse_url,
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
use sha2::digest::Output;
use std::cmp::{Ordering, Reverse};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::{self, Display, Formatter};
use std::io::Read;
use std::ops::Deref;
use std::path::Path;
use std::str::FromStr;

/// Inventory currently stores [`GemVersion`] but we want real types like [`RubyDownloadVersion`](crate::RubyDownloadVersion)
pub fn version_target<T, E>(
    inventory: &RubyInventory,
) -> Result<BTreeSet<(T, BuildTarget)>, NonEmpty<E>>
where
    T: for<'a> TryFrom<&'a GemVersion, Error = E> + Ord,
{
    let mut held: BTreeSet<(T, BuildTarget)> = BTreeSet::new();
    let mut errors = Vec::<T::Error>::new();
    for (version, target) in inventory.version_targets() {
        match T::try_from(&version) {
            Ok(v) => {
                held.insert((v, target));
            }
            Err(e) => {
                errors.push(e);
            }
        };
    }

    if let Ok(e) = NonEmpty::try_from(errors) {
        Err(e)
    } else {
        Ok(held)
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct ArtifactMetadata {
    pub timestamp: DateTime<Utc>,
    pub distro_version: DistroVersion,
}

type RawInventory = Inventory<GemVersion, Sha256, ArtifactMetadata>;
type RawArtifact = Artifact<GemVersion, Sha256, ArtifactMetadata>;

/// Unique ordered version/target pairs
///
/// For example ruby-4.0.10 (version) on Ubuntu 26.04 for ARM (target)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionTarget {
    pub version: GemVersion,
    pub target: BuildTarget,
}

/// Sort descending so that larger version numbers are shown first.
impl Ord for VersionTarget {
    fn cmp(&self, other: &Self) -> Ordering {
        (Reverse(&self.version), &self.target).cmp(&(Reverse(&other.version), &other.target))
    }
}

impl PartialOrd for VersionTarget {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Display for VersionTarget {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} on {} ({})",
            self.version, self.target.base_image, self.target.arch
        )
    }
}

/// One checked entry in an inventory file
///
/// See [`ArtifactError`] for a list of checks
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RubyArtifact {
    url: Url,
    base_image: BaseImage,
    artifact: RawArtifact,
}

impl RubyArtifact {
    pub fn url(&self) -> &Url {
        &self.url
    }

    pub fn key(&self) -> VersionTarget {
        VersionTarget {
            version: self.artifact.version.clone(),
            target: BuildTarget {
                base_image: self.base_image.clone(),
                arch: self.artifact.arch,
            },
        }
    }
}

impl Deref for RubyArtifact {
    type Target = RawArtifact;

    fn deref(&self) -> &Self::Target {
        &self.artifact
    }
}

impl From<RubyArtifact> for RawArtifact {
    fn from(artifact: RubyArtifact) -> Self {
        artifact.artifact
    }
}

impl TryFrom<RawArtifact> for RubyArtifact {
    type Error = ArtifactError;

    fn try_from(artifact: RawArtifact) -> Result<Self, Self::Error> {
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
        let key = VersionTarget {
            version,
            target: BuildTarget {
                base_image: base_image.clone(),
                arch: artifact.arch,
            },
        };
        let url = parse_url(&artifact.url).map_err(|source| ArtifactError::InvalidUrl {
            key: key.clone(),
            source,
        })?;
        let bucket = parse_url(S3_BASE_URL).expect("internal constant should be a valid URL");
        if url.origin() != bucket.origin() {
            return Err(ArtifactError::UrlOutsideBucket {
                key,
                url: Box::new(url),
            });
        }
        Ok(Self {
            url,
            base_image,
            artifact,
        })
    }
}

/// Conditions that would cause an invalid entry in the inventory file
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
        key: VersionTarget,
        source: UrlParseError,
    },

    #[error("{key}: url {url} is not under {S3_BASE_URL}")]
    UrlOutsideBucket { key: VersionTarget, url: Box<Url> },
}

/// Represents an inventory file in memory e.g. `ruby_inventory.toml`
///
/// Versions (e.g. `4.0.10`) per target (e.g. Ubuntu 26.04 on ARM) are
/// guaranteed to be unique and consistently ordered in memory
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RubyInventory(BTreeMap<VersionTarget, RubyArtifact>);

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

    pub fn version_targets(&self) -> BTreeSet<(GemVersion, BuildTarget)> {
        self.0
            .keys()
            .map(|x| (x.version.clone(), x.target.clone()))
            .collect()
    }

    pub fn artifacts(&self) -> impl ExactSizeIterator<Item = &RubyArtifact> {
        self.0.values()
    }
}

impl TryFrom<RawInventory> for RubyInventory {
    type Error = InventoryError;

    fn try_from(inventory: RawInventory) -> Result<Self, Self::Error> {
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
                .parse::<RawInventory>()
                .map_err(InventoryError::Parse)?,
        )
    }
}

impl Display for RubyInventory {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        RawInventory {
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
    Duplicate(VersionTarget),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InventoryChange {
    Added(RubyArtifact),
    Removed(RubyArtifact),
    Replaced {
        old: Box<RubyArtifact>,
        new: Box<RubyArtifact>,
    },
}

impl InventoryChange {
    pub fn key(&self) -> VersionTarget {
        match self {
            Self::Added(artifact) | Self::Removed(artifact) => artifact.key(),
            Self::Replaced { new, .. } => new.key(),
        }
    }
}

impl Display for InventoryChange {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let label = match self {
            Self::Added(_) => "added",
            Self::Removed(_) => "removed",
            Self::Replaced { .. } => "replaced",
        };
        write!(f, "{label} {}", self.key())
    }
}

/// See what changed between two inventory files
///
/// Returns `None` when they contain identical information
pub fn changes(old: &RubyInventory, new: &RubyInventory) -> Option<NonEmpty<InventoryChange>> {
    let mut old = old.0.iter().peekable();
    let mut new = new.0.iter().peekable();
    let mut found = Vec::new();
    loop {
        let order = match (old.peek(), new.peek()) {
            (None, None) => break,
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (Some((old_key, _)), Some((new_key, _))) => old_key.cmp(new_key),
        };
        match order {
            Ordering::Less => {
                let (_, removed) = old.next().expect("peeked");
                found.push(InventoryChange::Removed(removed.clone()));
            }
            Ordering::Greater => {
                let (_, added) = new.next().expect("peeked");
                found.push(InventoryChange::Added(added.clone()));
            }
            Ordering::Equal => {
                let ((_, o), (_, n)) = (old.next().expect("peeked"), new.next().expect("peeked"));
                if o != n {
                    found.push(InventoryChange::Replaced {
                        old: Box::new(o.clone()),
                        new: Box::new(n.clone()),
                    });
                }
            }
        }
    }
    NonEmpty::try_from(found).ok()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AllowedChanges {
    AddOnly,
    Any,
}

#[derive(Debug, thiserror::Error)]
pub enum InventoryCheckError {
    #[error("old inventory: {0}")]
    Old(#[source] InventoryError),

    #[error("new inventory: {0}")]
    New(#[source] InventoryError),

    #[error(
        "Expected the inventory change to only add artifacts, but it also removes or replaces some:\n{}\n\nPass `--allow-inventory-change` (the `allow inventory change` PR label) to allow this.",
        unexpected.iter().map(|change| format!("- {change}")).collect::<Vec<_>>().join("\n")
    )]
    UnexpectedChanges {
        unexpected: NonEmpty<InventoryChange>,
    },

    #[error(transparent)]
    Checksum(ChecksumError),
}

#[derive(Debug, thiserror::Error)]
pub enum ChecksumError {
    #[error("{url}: cannot create a temporary directory: {source}")]
    TempDir {
        url: Box<Url>,
        source: std::io::Error,
    },

    #[error("{url}: {source}")]
    Download { url: Box<Url>, source: Error },

    #[error("{url}: cannot hash the downloaded file: {source}")]
    Hash {
        url: Box<Url>,
        source: std::io::Error,
    },

    #[error("checksum task failed: {0}")]
    Task(#[source] tokio::task::JoinError),

    #[error(
        "{url}: checksum mismatch, expected sha256:{} got sha256:{}",
        hex::encode(&expected.value),
        hex::encode(actual)
    )]
    Mismatch {
        url: Box<Url>,
        expected: Checksum<Sha256>,
        actual: Output<Sha256>,
    },
}

/// Validate an inventory change before merge
///
/// This is used in automation to preserve the integrity of an inventory file e.g. `ruby_inventory.toml`.
/// Added entries are checked by downloading and validating their checksums. Existing entries are
/// assumed to have valid checksums.
///
/// - [`AllowedChanges::AddOnly`]: Errors on removals or changes. This is an automation guard so that a network outage
///   doesn't result in an inventory accidentally removing valid artifacts from the inventory.
/// - [`AllowedChanges::Any`]: Removals are allowed. Changes are validated for correctness.
///
/// See [`InventoryCheckError`] for other error conditions
pub async fn inventory_check(
    old: &str,
    new: &str,
    allowed: AllowedChanges,
) -> Result<Option<NonEmpty<InventoryChange>>, NonEmpty<InventoryCheckError>> {
    let Some(ChangeReview {
        changes,
        mut errors,
    }) = review_changes(old, new, allowed)?
    else {
        return Ok(None);
    };

    if let Err(failed) = verify_checksums(artifacts_to_verify(&changes)).await {
        errors.extend(failed.map(InventoryCheckError::Checksum));
    }

    match NonEmpty::try_from(errors) {
        Ok(errors) => Err(errors),
        Err(_) => Ok(Some(changes)),
    }
}

#[derive(Debug)]
struct ChangeReview {
    changes: NonEmpty<InventoryChange>,
    errors: Vec<InventoryCheckError>,
}

fn review_changes(
    old: &str,
    new: &str,
    allowed: AllowedChanges,
) -> Result<Option<ChangeReview>, NonEmpty<InventoryCheckError>> {
    let (old, new) = match (old.parse::<RubyInventory>(), new.parse::<RubyInventory>()) {
        (Ok(old), Ok(new)) => (old, new),
        (Err(old), Ok(_)) => return Err(NonEmpty::new(InventoryCheckError::Old(old))),
        (Ok(_), Err(new)) => return Err(NonEmpty::new(InventoryCheckError::New(new))),
        (Err(old), Err(new)) => {
            let mut errors = NonEmpty::new(InventoryCheckError::Old(old));
            errors.push(InventoryCheckError::New(new));
            return Err(errors);
        }
    };
    let Some(changes) = changes(&old, &new) else {
        return Ok(None);
    };

    let mut errors = Vec::new();
    if allowed == AllowedChanges::AddOnly {
        let unexpected = changes
            .iter()
            .filter(|change| !matches!(change, InventoryChange::Added(_)))
            .cloned()
            .collect::<Vec<_>>();
        if let Ok(unexpected) = NonEmpty::try_from(unexpected) {
            errors.push(InventoryCheckError::UnexpectedChanges { unexpected });
        }
    }
    Ok(Some(ChangeReview { changes, errors }))
}

fn artifacts_to_verify(changes: &NonEmpty<InventoryChange>) -> impl Iterator<Item = &RubyArtifact> {
    changes.iter().filter_map(|change| match change {
        InventoryChange::Added(artifact) => Some(artifact),
        InventoryChange::Replaced { new, .. } => Some(new.as_ref()),
        InventoryChange::Removed(_) => None,
    })
}

/// Download each artifact, generate its checksum, error if the value differs from the provided inventory.
pub async fn verify_checksums<'a>(
    artifacts: impl IntoIterator<Item = &'a RubyArtifact>,
) -> Result<(), NonEmpty<ChecksumError>> {
    let mut set = tokio::task::JoinSet::new();
    for artifact in artifacts.into_iter().cloned() {
        set.spawn(verify_checksum(artifact));
    }

    let mut errors = Vec::new();
    while let Some(joined) = set.join_next().await {
        match joined {
            Ok(Ok(())) => {}
            Ok(Err(error)) => errors.push(error),
            Err(join_error) => errors.push(ChecksumError::Task(join_error)),
        }
    }

    match NonEmpty::try_from(errors) {
        Ok(errors) => Err(errors),
        Err(_) => Ok(()),
    }
}

async fn verify_checksum(artifact: RubyArtifact) -> Result<(), ChecksumError> {
    let actual = download_sha256(artifact.url()).await?.sha256;
    if actual.as_slice() == artifact.checksum.value {
        Ok(())
    } else {
        Err(ChecksumError::Mismatch {
            url: Box::new(artifact.url().clone()),
            expected: artifact.checksum.clone(),
            actual,
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum DownloadArtifactError {
    #[error(transparent)]
    Download(#[from] ChecksumError),

    #[error(transparent)]
    Artifact(#[from] ArtifactError),

    #[error("{url}: response has no valid Last-Modified header")]
    LastModified { url: Box<Url> },
}

/// Download `url` and build the inventory entry for `version` on `target`, with the sha256 of the download
pub async fn download_artifact(
    url: &Url,
    version: &GemVersion,
    target: BuildTarget,
) -> Result<RubyArtifact, DownloadArtifactError> {
    let download = download_sha256(url).await?;
    let timestamp = last_modified(url, &download.headers)?;
    Ok(build_artifact(
        url,
        version,
        target,
        &download.sha256,
        timestamp,
    )?)
}

fn last_modified(
    url: &Url,
    headers: &reqwest::header::HeaderMap,
) -> Result<DateTime<Utc>, DownloadArtifactError> {
    let missing = || DownloadArtifactError::LastModified {
        url: Box::new(url.clone()),
    };
    let value = headers
        .get(reqwest::header::LAST_MODIFIED)
        .ok_or_else(missing)?
        .to_str()
        .map_err(|_| missing())?;
    DateTime::parse_from_rfc2822(value)
        .map(|time| time.with_timezone(&Utc))
        .map_err(|_| missing())
}

pub fn build_artifact(
    url: &Url,
    version: &GemVersion,
    target: BuildTarget,
    sha256: &Output<Sha256>,
    timestamp: DateTime<Utc>,
) -> Result<RubyArtifact, ArtifactError> {
    RubyArtifact::try_from(RawArtifact {
        version: version.clone(),
        os: Os::Linux,
        arch: target.arch,
        url: url.to_string(),
        checksum: format!("sha256:{}", hex::encode(sha256))
            .parse()
            .expect("a hex encoded sha256 is a valid checksum"),
        metadata: ArtifactMetadata {
            timestamp,
            distro_version: target.base_image.distro_version(),
        },
    })
}

struct ChecksumDownload {
    sha256: Output<Sha256>,
    headers: reqwest::header::HeaderMap,
}

async fn download_sha256(url: &Url) -> Result<ChecksumDownload, ChecksumError> {
    let boxed_url = || Box::new(url.clone());
    let temp = tempfile::tempdir().map_err(|source| ChecksumError::TempDir {
        url: boxed_url(),
        source,
    })?;
    let path = temp.path().join("file.tar");

    let headers = download_tar(url.as_str(), &TarDownloadPath(path.clone()))
        .await
        .map_err(|source| ChecksumError::Download {
            url: boxed_url(),
            source,
        })?;

    // Computing a file Hash is CPU bound <https://docs.rs/tokio/latest/tokio/index.html#cpu-bound-tasks-and-blocking-code>
    let sha256 =
        tokio::task::spawn_blocking(move || fs::File::open(&path).and_then(digest::<Sha256>))
            .await
            .map_err(ChecksumError::Task)?
            .map_err(|source| ChecksumError::Hash {
                url: boxed_url(),
                source,
            })?;
    Ok(ChecksumDownload { sha256, headers })
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
    use sha2::Digest;

    fn artifact(version: &str, base_image: &str, arch: Arch) -> RawArtifact {
        RawArtifact {
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

    fn inventory(artifacts: Vec<RawArtifact>) -> RubyInventory {
        RubyInventory::try_from(RawInventory { artifacts }).unwrap()
    }

    fn keys(inventory: &RubyInventory) -> Vec<String> {
        inventory
            .artifacts()
            .map(|artifact| artifact.key().to_string())
            .collect()
    }

    fn unsorted_toml(artifacts: Vec<RawArtifact>) -> String {
        RawInventory { artifacts }.to_string()
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
        let error = RubyInventory::try_from(RawInventory {
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

    fn change_keys(changes: Option<NonEmpty<InventoryChange>>) -> Vec<String> {
        changes
            .iter()
            .flat_map(|changes| changes.iter())
            .map(ToString::to_string)
            .collect()
    }

    #[test]
    fn same_inventory_has_no_changes() {
        let old = inventory(vec![artifact("3.4.0", "heroku-24", Arch::Amd64)]);
        let new = inventory(vec![artifact("3.4.0", "heroku-24", Arch::Amd64)]);
        assert!(changes(&old, &new).is_none());
    }

    #[test]
    fn changes_lists_added_removed_and_replaced_in_inventory_order() {
        let old = inventory(vec![
            artifact("3.3.0", "heroku-24", Arch::Amd64),
            artifact("3.4.0", "heroku-24", Arch::Amd64),
            artifact("3.4.0", "heroku-24", Arch::Arm64),
        ]);
        let mut rebuilt = artifact("3.4.0", "heroku-24", Arch::Amd64);
        rebuilt.checksum =
            "sha256:7bebeee1b9128bdbb290331b813fa01cf43e30cd0098286f7de011796cb8eee5"
                .parse()
                .unwrap();
        let new = inventory(vec![
            artifact("3.3.0", "heroku-24", Arch::Amd64),
            rebuilt,
            artifact("3.4.1", "heroku-24", Arch::Amd64),
        ]);
        assert_eq!(
            change_keys(changes(&old, &new)),
            vec![
                "added 3.4.1 on heroku-24 (amd64)",
                "replaced 3.4.0 on heroku-24 (amd64)",
                "removed 3.4.0 on heroku-24 (arm64)",
            ]
        );
    }

    #[tokio::test]
    async fn check_with_no_changes_downloads_nothing() {
        let contents = inventory(vec![artifact("3.4.0", "heroku-24", Arch::Amd64)]).to_string();
        let changes = inventory_check(&contents, &contents, AllowedChanges::AddOnly)
            .await
            .unwrap();
        assert!(changes.is_none());
    }

    fn review_keys(review: &ChangeReview) -> Vec<String> {
        review.changes.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn review_reports_removed_and_replaced_artifacts_with_the_changes() {
        let old = inventory(vec![
            artifact("3.4.0", "heroku-24", Arch::Amd64),
            artifact("3.4.0", "heroku-24", Arch::Arm64),
        ])
        .to_string();
        let mut rebuilt = artifact("3.4.0", "heroku-24", Arch::Amd64);
        rebuilt.url = format!("{S3_BASE_URL}/heroku-24/amd64/ruby-3.4.0-7bebeee.tgz");
        let new = inventory(vec![rebuilt, artifact("3.4.1", "heroku-24", Arch::Amd64)]).to_string();
        let review = review_changes(&old, &new, AllowedChanges::AddOnly)
            .unwrap()
            .unwrap();
        assert_eq!(
            review_keys(&review),
            vec![
                "added 3.4.1 on heroku-24 (amd64)",
                "replaced 3.4.0 on heroku-24 (amd64)",
                "removed 3.4.0 on heroku-24 (arm64)",
            ]
        );
        assert_eq!(
            review
                .errors
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>(),
            vec![indoc::indoc! {"
                Expected the inventory change to only add artifacts, but it also removes or replaces some:
                - replaced 3.4.0 on heroku-24 (amd64)
                - removed 3.4.0 on heroku-24 (arm64)

                Pass `--allow-inventory-change` (the `allow inventory change` PR label) to allow this."}]
        );
        assert_eq!(
            artifacts_to_verify(&review.changes)
                .map(|artifact| artifact.key().to_string())
                .collect::<Vec<_>>(),
            vec!["3.4.1 on heroku-24 (amd64)", "3.4.0 on heroku-24 (amd64)"]
        );
    }

    #[test]
    fn review_allows_removal_when_any_change_is_allowed() {
        let old = inventory(vec![
            artifact("3.4.0", "heroku-24", Arch::Amd64),
            artifact("3.4.0", "heroku-24", Arch::Arm64),
        ])
        .to_string();
        let new = inventory(vec![artifact("3.4.0", "heroku-24", Arch::Amd64)]).to_string();
        let review = review_changes(&old, &new, AllowedChanges::Any)
            .unwrap()
            .unwrap();
        assert_eq!(
            review_keys(&review),
            vec!["removed 3.4.0 on heroku-24 (arm64)"]
        );
        assert!(review.errors.is_empty());
        assert_eq!(artifacts_to_verify(&review.changes).count(), 0);
    }

    #[tokio::test]
    async fn check_names_which_inventory_failed_to_parse() {
        let errors = inventory_check("", "artifacts = []\n", AllowedChanges::AddOnly)
            .await
            .unwrap_err();
        assert_eq!(errors.len(), 1);
        assert!(matches!(errors.first(), InventoryCheckError::Old(_)));
    }

    #[test]
    fn review_reports_both_inventories_when_neither_parses() {
        let errors = review_changes("", "", AllowedChanges::AddOnly).unwrap_err();
        assert_eq!(errors.len(), 2);
        assert!(matches!(errors[0], InventoryCheckError::Old(_)));
        assert!(matches!(errors[1], InventoryCheckError::New(_)));
    }

    fn local_artifact(base: &str, version: &str, arch: Arch, path: &str) -> RubyArtifact {
        let mut artifact = RubyArtifact::try_from(artifact(version, "heroku-24", arch)).unwrap();
        artifact.url = parse_url(&format!("{base}{path}")).unwrap();
        artifact
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn verify_checksums_reports_every_failed_artifact() {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let base = format!("http://{}", server.server_addr());
        std::thread::spawn(move || {
            for request in server.incoming_requests() {
                let response = if request.url() == "/missing.tgz" {
                    tiny_http::Response::from_string("").with_status_code(404)
                } else {
                    tiny_http::Response::from_string("not the expected bytes")
                };
                let _ = request.respond(response);
            }
        });

        let mismatched = local_artifact(&base, "3.4.0", Arch::Amd64, "/mismatched.tgz");
        let missing = local_artifact(&base, "3.4.0", Arch::Arm64, "/missing.tgz");

        let errors = verify_checksums([&mismatched, &missing]).await.unwrap_err();
        assert_eq!(errors.len(), 2);
        assert!(errors.iter().any(|error| matches!(
            error,
            ChecksumError::Mismatch { url, .. } if **url == *mismatched.url()
        )));
        assert!(errors.iter().any(|error| matches!(
            error,
            ChecksumError::Download { url, .. } if **url == *missing.url()
        )));
    }

    #[test]
    fn build_artifact_records_the_target_checksum_and_url() {
        let url = parse_url(&format!("{S3_BASE_URL}/heroku-24/arm64/ruby-3.4.0.tgz")).unwrap();
        let target = BuildTarget {
            base_image: BaseImage::new("heroku-24").unwrap(),
            arch: Arch::Arm64,
        };
        let sha256 = Sha256::digest(b"ruby");
        let timestamp = "2026-04-22T16:44:16Z".parse().unwrap();

        let artifact =
            build_artifact(&url, &"3.4.0".parse().unwrap(), target, &sha256, timestamp).unwrap();

        assert_eq!(artifact.key().to_string(), "3.4.0 on heroku-24 (arm64)");
        assert_eq!(artifact.url(), &url);
        assert_eq!(artifact.checksum.value, sha256.as_slice());
        assert_eq!(artifact.metadata.timestamp, timestamp);
    }

    #[test]
    fn last_modified_reads_the_s3_header() {
        let url = parse_url(&format!("{S3_BASE_URL}/heroku-24/amd64/ruby-4.0.10.tgz")).unwrap();
        let mut headers = reqwest::header::HeaderMap::new();
        assert!(matches!(
            last_modified(&url, &headers),
            Err(DownloadArtifactError::LastModified { .. })
        ));

        headers.insert(
            reqwest::header::LAST_MODIFIED,
            "Wed, 18 Nov 2026 23:41:39 GMT".parse().unwrap(),
        );
        assert_eq!(
            last_modified(&url, &headers).unwrap().to_rfc3339(),
            "2026-11-18T23:41:39+00:00"
        );
    }

    #[test]
    fn build_artifact_rejects_an_arch_the_base_image_does_not_build() {
        let url = parse_url(&format!("{S3_BASE_URL}/heroku-22/arm64/ruby-3.4.0.tgz")).unwrap();
        let target = BuildTarget {
            base_image: BaseImage::new("heroku-22").unwrap(),
            arch: Arch::Arm64,
        };

        let error = build_artifact(
            &url,
            &"3.4.0".parse().unwrap(),
            target,
            &Sha256::digest(b"ruby"),
            Utc::now(),
        )
        .unwrap_err();

        assert!(matches!(error, ArtifactError::UnsupportedArch { .. }));
    }

    #[test]
    fn changes_lists_artifacts_past_the_end_of_the_other_inventory() {
        let old = inventory(vec![artifact("3.3.0", "heroku-24", Arch::Amd64)]);
        let new = inventory(vec![artifact("3.4.0", "heroku-24", Arch::Amd64)]);
        assert_eq!(
            change_keys(changes(&old, &new)),
            vec![
                "added 3.4.0 on heroku-24 (amd64)",
                "removed 3.3.0 on heroku-24 (amd64)",
            ]
        );
        assert_eq!(
            change_keys(changes(&new, &RubyInventory::default())),
            vec!["removed 3.4.0 on heroku-24 (amd64)"]
        );
    }
}
