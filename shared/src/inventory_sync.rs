//! ⁉️😄⁉️ - module purpose: how an S3 build becomes an inventory artifact, and which URL it records

use crate::{
    ArtifactKey, ArtifactMetadata, Error, InventoryError, RubyArtifact, RubyInventory,
    TarDownloadPath, download_tar, s3_last_modified, sha256_from_path,
};
use gem_version::GemVersion;
use libherokubuildpack::inventory::artifact::{Artifact, Os};
use libherokubuildpack::inventory::checksum::Checksum;
use reqwest::Url;
use sha2::Sha256;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::Semaphore;
use tokio::task::JoinSet;

const MAX_CONCURRENT_LOOKUPS: usize = 16;
const LOOKUPS_PER_SAVE: usize = 64;

#[derive(Debug, Clone)]
pub struct Wanted {
    pub key: ArtifactKey,
    pub url: Url,
}

#[derive(Debug, Default)]
pub struct SyncReport {
    pub added: Vec<ArtifactKey>,
    pub missing: Vec<ArtifactKey>,
    pub errors: Vec<SyncError>,
}

impl SyncReport {
    fn extend(&mut self, other: SyncReport) {
        self.added.extend(other.added);
        self.missing.extend(other.missing);
        self.errors.extend(other.errors);
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SaveError {
    #[error(transparent)]
    Inventory(#[from] InventoryError),

    #[error(transparent)]
    Write(#[from] std::io::Error),
}

#[derive(Debug, thiserror::Error)]
#[error("{key}: {source}")]
pub struct SyncError {
    pub key: ArtifactKey,
    #[source]
    pub source: Error,
}

/// ⁉️😄⁉️ - cost: downloads each new build once to hash it, and artifacts already in the inventory are not looked up
pub async fn sync_inventory(
    inventory: &mut RubyInventory,
    wanted: Vec<Wanted>,
) -> Result<SyncReport, InventoryError> {
    let semaphore = Arc::new(Semaphore::new(MAX_CONCURRENT_LOOKUPS));
    let mut set = JoinSet::new();
    for wanted in wanted.into_iter().filter(|w| !inventory.contains(&w.key)) {
        let semaphore = semaphore.clone();
        set.spawn(async move {
            let _permit = semaphore.acquire_owned().await;
            let result = locate(&wanted).await.and_then(|found| {
                found
                    .map(|artifact| {
                        RubyArtifact::try_from(artifact).map_err(|e| Error::Artifact(Box::new(e)))
                    })
                    .transpose()
            });
            (wanted.key, result)
        });
    }

    let mut report = SyncReport::default();
    while let Some(joined) = set.join_next().await {
        match joined {
            Ok((key, Ok(Some(artifact)))) => {
                inventory.insert(artifact)?;
                report.added.push(key);
            }
            Ok((key, Ok(None))) => report.missing.push(key),
            Ok((key, Err(source))) => report.errors.push(SyncError { key, source }),
            Err(join_error) => std::panic::resume_unwind(join_error.into_panic()),
        }
    }
    report.added.sort();
    report.missing.sort();
    report.errors.sort_by(|a, b| a.key.cmp(&b.key));
    Ok(report)
}

/// ⁉️😄⁉️ - why: a run that is stopped part way still keeps the artifacts it found
pub async fn sync_inventory_file(
    path: &Path,
    inventory: &mut RubyInventory,
    wanted: Vec<Wanted>,
) -> Result<SyncReport, SaveError> {
    let mut report = SyncReport::default();
    for batch in wanted.chunks(LOOKUPS_PER_SAVE) {
        report.extend(sync_inventory(inventory, batch.to_vec()).await?);
        fs_err::write(path, inventory.to_string())?;
    }
    fs_err::write(path, inventory.to_string())?;
    report.added.sort();
    report.missing.sort();
    report.errors.sort_by(|a, b| a.key.cmp(&b.key));
    Ok(report)
}

/// ⁉️😄⁉️ - why the sha7 copy is preferred over the URL that was downloaded
async fn locate(
    wanted: &Wanted,
) -> Result<Option<Artifact<GemVersion, Sha256, ArtifactMetadata>>, Error> {
    let Some(last_modified) = s3_last_modified(wanted.url.clone()).await? else {
        return Ok(None);
    };
    let checksum = download_checksum(&wanted.url).await?;
    let sha_seven_url = sha_seven_url(&wanted.url, &checksum)?;
    let (url, timestamp) = match s3_last_modified(sha_seven_url.clone()).await? {
        Some(sha_seven_last_modified) => (sha_seven_url, sha_seven_last_modified),
        None => (wanted.url.clone(), last_modified),
    };
    Ok(Some(Artifact {
        version: wanted.key.version.clone(),
        os: Os::Linux,
        arch: wanted.key.target.arch,
        url: url.to_string(),
        checksum,
        metadata: ArtifactMetadata {
            timestamp,
            distro_version: wanted.key.target.base_image.distro_version(),
        },
    }))
}

async fn download_checksum(url: &Url) -> Result<Checksum<Sha256>, Error> {
    let temp = tempfile::tempdir().map_err(Error::FsError)?;
    let path = temp.path().join("download.tgz");
    download_tar(url.as_str(), &TarDownloadPath(path.clone())).await?;
    let hex = tokio::task::spawn_blocking(move || sha256_from_path(&path))
        .await
        .map_err(Error::HashTask)??;
    Ok(format!("sha256:{hex}")
        .parse()
        .expect("sha256_from_path returns 64 hex characters"))
}

/// ⁉️😄⁉️ - why a build also uploads a copy named for its checksum
pub fn sha_seven_url(url: &Url, checksum: &Checksum<Sha256>) -> Result<Url, Error> {
    let name = url
        .path_segments()
        .and_then(|mut segments| segments.next_back())
        .and_then(|name| name.strip_suffix(".tgz"))
        .ok_or_else(|| Error::NotATarballUrl(url.clone()))?;
    let sha_seven = &hex::encode(&checksum.value)[..7];
    let file_name = format!("{name}-{sha_seven}.tgz");
    let mut sha_seven_url = url.clone();
    sha_seven_url
        .path_segments_mut()
        .map_err(|()| Error::NotATarballUrl(url.clone()))?
        .pop()
        .push(&file_name);
    Ok(sha_seven_url)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BaseImage, BuildTarget, S3_BASE_URL, parse_url};
    use chrono::{DateTime, Utc};
    use libherokubuildpack::inventory::artifact::Arch;
    use std::str::FromStr;

    fn checksum(prefix: &str) -> Checksum<Sha256> {
        format!("sha256:{prefix}{}", "0".repeat(64 - prefix.len()))
            .parse()
            .unwrap()
    }

    #[test]
    fn sha_seven_url_appends_to_the_file_name() {
        let url = parse_url(&format!("{S3_BASE_URL}/heroku-24/amd64/ruby-3.4.7.tgz")).unwrap();
        assert_eq!(
            sha_seven_url(&url, &checksum("fdf0513")).unwrap().as_str(),
            format!("{S3_BASE_URL}/heroku-24/amd64/ruby-3.4.7-fdf0513.tgz")
        );
    }

    #[test]
    fn sha_seven_url_rejects_a_non_tgz_url() {
        let url = parse_url(&format!("{S3_BASE_URL}/heroku-24/amd64/ruby-3.4.7.tar")).unwrap();
        assert!(sha_seven_url(&url, &checksum("fdf0513")).is_err());
    }

    const HELLO_SHA256: &str = "185f8db32271fe25f561a6fc938b2e264306ec304eda518007d1764826381969";

    struct Object {
        path: String,
        last_modified: &'static str,
    }

    fn serve(objects: Vec<Object>) -> Url {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let base = parse_url(&format!("http://{}/", server.server_addr())).unwrap();
        std::thread::spawn(move || {
            for request in server.incoming_requests() {
                let response = match objects.iter().find(|o| o.path == request.url()) {
                    Some(object) => tiny_http::Response::from_string("Hello").with_header(
                        format!("Last-Modified: {}", object.last_modified)
                            .parse::<tiny_http::Header>()
                            .unwrap(),
                    ),
                    None => tiny_http::Response::from_string("").with_status_code(403),
                };
                let _ = request.respond(response);
            }
        });
        base
    }

    fn wanted(url: Url) -> Wanted {
        Wanted {
            key: ArtifactKey {
                version: GemVersion::from_str("3.4.7").unwrap(),
                target: BuildTarget {
                    base_image: BaseImage::new("heroku-24").unwrap(),
                    arch: Arch::Amd64,
                },
            },
            url,
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn records_the_sha_seven_copy_when_it_exists() {
        let base = serve(vec![
            Object {
                path: "/ruby-3.4.7.tgz".to_owned(),
                last_modified: "Wed, 03 Apr 2024 14:40:03 GMT",
            },
            Object {
                path: format!("/ruby-3.4.7-{}.tgz", &HELLO_SHA256[..7]),
                last_modified: "Thu, 04 Apr 2024 10:00:00 GMT",
            },
        ]);
        let artifact = locate(&wanted(base.join("ruby-3.4.7.tgz").unwrap()))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            artifact.url,
            base.join(&format!("ruby-3.4.7-{}.tgz", &HELLO_SHA256[..7]))
                .unwrap()
                .as_str()
        );
        assert_eq!(hex::encode(&artifact.checksum.value), HELLO_SHA256);
        assert_eq!(
            artifact.metadata.timestamp,
            "2024-04-04T10:00:00Z".parse::<DateTime<Utc>>().unwrap()
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn records_the_url_without_a_sha_seven_copy() {
        let base = serve(vec![Object {
            path: "/ruby-3.4.7.tgz".to_owned(),
            last_modified: "Wed, 03 Apr 2024 14:40:03 GMT",
        }]);
        let artifact = locate(&wanted(base.join("ruby-3.4.7.tgz").unwrap()))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(artifact.url, base.join("ruby-3.4.7.tgz").unwrap().as_str());
        assert_eq!(
            artifact.metadata.timestamp,
            "2024-04-03T14:40:03Z".parse::<DateTime<Utc>>().unwrap()
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_build_on_no_url_is_missing() {
        let base = serve(vec![]);
        let artifact = locate(&wanted(base.join("ruby-3.4.7.tgz").unwrap()))
            .await
            .unwrap();
        assert!(artifact.is_none());
    }

    #[tokio::test]
    async fn artifacts_already_in_the_inventory_are_not_looked_up() {
        let key = ArtifactKey {
            version: GemVersion::from_str("3.4.7").unwrap(),
            target: BuildTarget {
                base_image: BaseImage::new("heroku-24").unwrap(),
                arch: Arch::Amd64,
            },
        };
        let existing = Artifact {
            version: key.version.clone(),
            os: Os::Linux,
            arch: Arch::Amd64,
            url: format!("{S3_BASE_URL}/heroku-24/amd64/ruby-3.4.7.tgz"),
            checksum: "sha256:dd073bda5665e758c3e6f861a6df435175c8e8faf5ec75bc2afaab1e3eebb2c7"
                .parse()
                .unwrap(),
            metadata: ArtifactMetadata {
                timestamp: "2026-04-22T16:44:16Z".parse().unwrap(),
                distro_version: key.target.base_image.distro_version(),
            },
        };
        let mut inventory =
            RubyInventory::new(vec![RubyArtifact::try_from(existing).unwrap()]).unwrap();
        let unreachable = parse_url("http://127.0.0.1:1/ruby-3.4.7.tgz").unwrap();
        let report = sync_inventory(
            &mut inventory,
            vec![Wanted {
                key,
                url: unreachable,
            }],
        )
        .await
        .unwrap();
        assert!(report.added.is_empty());
        assert!(report.missing.is_empty());
        assert!(report.errors.is_empty());
        assert_eq!(inventory.artifacts().len(), 1);
    }
}
