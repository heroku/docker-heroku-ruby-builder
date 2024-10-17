use crate::base_image::DistroVersion;
use crate::{download_tar, Error, TarDownloadPath};
use chrono::{DateTime, Utc};
use fs2::FileExt;
use gem_version::GemVersion;
use libherokubuildpack::inventory::checksum::Checksum;
use libherokubuildpack::inventory::{self, Inventory};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::borrow::Borrow;
use std::io::Read;
use std::io::Write;
use std::path::Path;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ArtifactMetadata {
    pub timestamp: DateTime<Utc>,
    pub distro_version: DistroVersion,
}

/// ```
/// use shared::inventory_check;
///
/// let contents = r#"
/// [[artifacts]]
/// version = "9.4.8.0"
/// os = "linux"
/// arch = "amd64"
/// url = "https://heroku-buildpack-ruby.s3.us-east-1.amazonaws.com/heroku-24/ruby-3.1.4-jruby-9.4.8.0.tgz"
/// checksum = "sha256:815b31d2b204a524bf74aabae341bf85353add4d1128d5d276d08fa5e8ff3c39"
///
/// [artifacts.metadata]
/// timestamp = "2024-07-24T16:17:35.341413Z"
/// distro_version = "24.04"
/// "#;
/// inventory_check(contents).unwrap();
/// ```
pub fn inventory_check(contents: &str) -> Result<(), Error> {
    if contents.trim().is_empty() {
        return Ok(());
    }

    let inventory = contents
        .parse::<Inventory<GemVersion, Sha256, ArtifactMetadata>>()
        .map_err(|e| Error::Other(format!("Could not parse inventory. Error: {e}")))?;

    let results = inventory
        .artifacts
        .par_iter()
        .map(|artifact| {
            let temp = tempfile::tempdir().expect("Tempdir");
            let dir = temp.path();
            let path = dir.join("file.tar");

            download_tar(&artifact.url, &TarDownloadPath(path.clone()))
                .map_err(|e| format!("Error {e}"))
                .and_then(|_| {
                    sha256_from_path(&path)
                        .map_err(|e| format!("Error {e}"))
                        .and_then(|checksum_string| {
                            format!("sha256:{checksum_string}")
                                .parse()
                                .map_err(|e| format!("Error {e}"))
                        })
                })
                .and_then(|checksum: Checksum<Sha256>| {
                    if checksum == artifact.checksum {
                        Ok(())
                    } else {
                        Err(format!(
                            "Checksum mismatch for {url} expected {expected} got {actual}",
                            url = artifact.url,
                            expected = hex::encode(&artifact.checksum.value),
                            actual = hex::encode(&checksum.value)
                        ))
                    }
                })
        })
        .collect::<Vec<Result<(), String>>>();

    if results.iter().any(|r| r.is_err()) {
        Err(Error::Other(
            results
                .iter()
                .map(|r| r.as_ref().unwrap_err().borrow())
                .collect::<Vec<&str>>()
                .join("\n"),
        ))
    } else {
        Ok(())
    }
}

fn atomic_file_contents<F, T>(path: &Path, f: F) -> Result<T, Box<dyn std::error::Error>>
where
    F: FnOnce(&mut std::fs::File, &str) -> Result<T, Box<dyn std::error::Error>>,
{
    fs_err::create_dir_all(
        path.parent().ok_or_else(|| {
            Error::Other(format!("Cannot determine parent from {}", path.display()))
        })?,
    )
    .map_err(Error::FsError)?;

    let mut file: std::fs::File = fs_err::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .open(path)?
        .into();
    file.lock_exclusive()?;
    use std::io::Seek;

    let mut contents = String::new();
    file.read_to_string(&mut contents).map_err(Error::FsError)?;
    file.rewind()?;
    let result: Result<T, Box<dyn std::error::Error>> = f(&mut file, &contents);
    file.unlock()?;
    result
}

pub fn atomic_inventory_update<F>(path: &Path, f: F) -> Result<(), Box<dyn std::error::Error>>
where
    F: FnOnce(
        &mut Inventory<GemVersion, Sha256, ArtifactMetadata>,
    ) -> Result<(), Box<dyn std::error::Error>>,
{
    atomic_file_contents(path, |file, contents| {
        let mut inventory = parse_inventory(contents)?;
        f(&mut inventory)?;
        write!(file, "{inventory}").map_err(Error::FsError)?;
        Ok(())
    })
}

fn parse_inventory(
    contents: &str,
) -> Result<Inventory<GemVersion, Sha256, ArtifactMetadata>, Error> {
    if contents.trim().is_empty() {
        Ok(Inventory {
            artifacts: Vec::new(),
        })
    } else {
        contents
            .parse::<Inventory<GemVersion, Sha256, ArtifactMetadata>>()
            .map_err(|e| Error::Other(format!("Error {e} parsing inventory:\n{contents}")))
    }
}

/// Returns the sha256 hash of the file at the given path
pub fn sha256_from_path(path: &Path) -> Result<String, Error> {
    digest::<Sha256>(fs_err::File::open(path).map_err(Error::FsError)?)
        .map(|digest| format!("{:x}", digest))
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

/// Raises an error if the same URL has a different checksum
///
/// This protects against the (reasonably) unlikely event that the same version generates a checksum with the same first 7 characters but a net different checksum.
/// While unlikely, it's still possible. If we didn't guard against this case, then it could break people's builds who are relying on the old checksum
/// no not change.
pub fn artifact_same_url_different_checksum(
    a: &inventory::artifact::Artifact<GemVersion, Sha256, ArtifactMetadata>,
    b: &inventory::artifact::Artifact<GemVersion, Sha256, ArtifactMetadata>,
) -> Result<(), Box<dyn std::error::Error>> {
    if a.url == b.url && a.checksum != b.checksum {
        Err(format!(
            "Duplicate url {url} has different checksums {a_checksum} != {b_checksum}",
            url = a.url,
            a_checksum = hex::encode(&a.checksum.value),
            b_checksum = hex::encode(&b.checksum.value)
        )
        .into())
    } else {
        Ok(())
    }
}

pub fn artifact_is_different(
    a: &inventory::artifact::Artifact<GemVersion, Sha256, ArtifactMetadata>,
    b: &inventory::artifact::Artifact<GemVersion, Sha256, ArtifactMetadata>,
) -> bool {
    a.version != b.version
        || a.arch != b.arch
        || a.metadata.distro_version != b.metadata.distro_version
}

#[cfg(test)]
mod test {
    use crate::BaseImage;
    use inventory::artifact::Artifact;
    use std::io::Write;
    use std::str::FromStr;

    use super::*;

    #[test]
    fn test_same_url_different_checksum_raises_error() {
        let a = Artifact {
            os: inventory::artifact::Os::Linux,
            arch: inventory::artifact::Arch::Amd64,
            version: GemVersion::from_str("1.0.0").unwrap(),
            checksum: "sha256:dd073bda5665e758c3e6f861a6df435175c8e8faf5ec75bc2afaab1e3eebb2c7"
                .parse()
                .unwrap(),
            metadata: ArtifactMetadata {
                timestamp: Utc::now(),
                distro_version: BaseImage::new("heroku-24").unwrap().distro_version(),
            },
            url: "https://example.com".to_string(),
        };

        let b = a.clone();
        artifact_same_url_different_checksum(&a, &b).unwrap();

        let mut b = a.clone();
        b.checksum = "sha256:7bebeee1b9128bdbb290331b813fa01cf43e30cd0098286f7de011796cb8eee5"
            .parse()
            .unwrap();
        assert!(artifact_same_url_different_checksum(&a, &b).is_err());
    }

    #[test]
    fn test_is_not_version_match() {
        let a = Artifact {
            os: inventory::artifact::Os::Linux,
            arch: inventory::artifact::Arch::Amd64,
            version: GemVersion::from_str("1.0.0").unwrap(),
            checksum: "sha256:dd073bda5665e758c3e6f861a6df435175c8e8faf5ec75bc2afaab1e3eebb2c7"
                .parse()
                .unwrap(),
            metadata: ArtifactMetadata {
                timestamp: Utc::now(),
                distro_version: BaseImage::new("heroku-24").unwrap().distro_version(),
            },
            url: "https://example.com".to_string(),
        };

        let b = a.clone();
        assert!(!artifact_is_different(&a, &b));

        let mut b = a.clone();
        b.version = GemVersion::from_str("1.0.1").unwrap();
        assert!(artifact_is_different(&a, &b));

        let mut b = a.clone();
        b.arch = inventory::artifact::Arch::Arm64;
        assert!(artifact_is_different(&a, &b));

        let mut b = a.clone();
        b.metadata.distro_version = BaseImage::new("heroku-22").unwrap().distro_version();
        assert!(artifact_is_different(&a, &b));
    }

    #[test]
    fn test_append_inventory() {
        let temp = tempfile::tempdir().expect("Tempdir");
        let path = temp.path().join("inventory.toml");
        let artifact = Artifact {
            os: inventory::artifact::Os::Linux,
            arch: inventory::artifact::Arch::Amd64,
            version: GemVersion::from_str("1.0.0").unwrap(),
            checksum: "sha256:dd073bda5665e758c3e6f861a6df435175c8e8faf5ec75bc2afaab1e3eebb2c7"
                .parse()
                .unwrap(),
            metadata: ArtifactMetadata {
                timestamp: Utc::now(),
                distro_version: BaseImage::new("heroku-24").unwrap().distro_version(),
            },
            url: "https://example.com".to_string(),
        };

        atomic_file_contents(&path, |file, contents| {
            let mut inventory = parse_inventory(contents)?;
            inventory.push(artifact.clone());
            write!(file, "{inventory}").expect("Writeable file");
            Ok(())
        })
        .unwrap();

        let inventory = parse_inventory(&fs_err::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(1, inventory.artifacts.len());

        atomic_file_contents(&path, |file, contents| {
            let mut inventory = parse_inventory(contents)?;
            inventory.push(artifact.clone());
            write!(file, "{inventory}").expect("Writeable file");
            Ok(())
        })
        .unwrap();
        let inventory = parse_inventory(&fs_err::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(2, inventory.artifacts.len());
    }
}
