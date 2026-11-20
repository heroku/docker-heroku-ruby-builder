use crate::base_image::DistroVersion;
use crate::{Error, TarDownloadPath, download_tar};
use chrono::{DateTime, Utc};
use fs_err::{self as fs};
use gem_version::GemVersion;
use libherokubuildpack::inventory::Inventory;
use libherokubuildpack::inventory::checksum::Checksum;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::io::Read;
use std::path::Path;

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct ArtifactMetadata {
    pub timestamp: DateTime<Utc>,
    pub distro_version: DistroVersion,
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
