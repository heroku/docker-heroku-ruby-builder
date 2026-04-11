use bullet_stream::global::print;
use clap::Parser;
use fs_err as fs;
use reqwest::Url;
use shared::{RubyDownloadVersion, S3_BASE_URL, build_matrix, output_ruby_tar_path};
use std::{
    error::Error,
    path::{Path, PathBuf},
    time::Duration,
};
use tokio::task::JoinSet;
use tokio::time::sleep;
use yaml_rust2::YamlLoader;

static RELEASES_URL: std::sync::LazyLock<Url> = std::sync::LazyLock::new(|| {
    Url::parse("https://raw.githubusercontent.com/ruby/www.ruby-lang.org/master/_data/releases.yml")
        .expect("valid releases URL constant")
});

const MAX_RETRY_ATTEMPTS: u8 = 3;
const RETRY_DELAY: Duration = Duration::from_secs(1);

#[derive(Parser, Debug)]
#[command(about = "Check for Ruby releases missing from Heroku S3")]
struct Args {
    /// Minimum Ruby version to check (e.g. 3.2.0). All releases >= this version will be checked.
    #[arg(long = "minimum-version", required = true)]
    minimum_version: RubyDownloadVersion,

    /// Path to write JSON output file containing versions that need builds
    #[arg(long = "output", required = true)]
    output: PathBuf,
}

async fn fetch_releases(url: &Url) -> Result<Vec<RubyDownloadVersion>, Box<dyn std::error::Error>> {
    let mut attempts = 0;
    loop {
        attempts += 1;
        match fetch_releases_inner(url).await {
            Ok(val) => return Ok(val),
            Err(error) => {
                if attempts >= MAX_RETRY_ATTEMPTS {
                    return Err(error);
                }
                sleep(RETRY_DELAY).await;
            }
        }
    }
}

async fn fetch_releases_inner(
    url: &Url,
) -> Result<Vec<RubyDownloadVersion>, Box<dyn std::error::Error>> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()?;
    let body = client
        .get(url.clone())
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;

    let docs = YamlLoader::load_from_str(&body)?;
    let releases = docs[0]
        .as_vec()
        .unwrap_or(&Vec::new())
        .iter()
        .filter_map(|entry| {
            entry["version"]
                .as_str()
                .and_then(|v| RubyDownloadVersion::new(v).ok())
        })
        .collect();
    Ok(releases)
}

fn version_gte(version: &RubyDownloadVersion, minimum: &RubyDownloadVersion) -> bool {
    let version_tuple = (version.major, version.minor, version.patch);
    let minimum_tuple = (minimum.major, minimum.minor, minimum.patch);
    version_tuple >= minimum_tuple
}

fn retain_releases_gte(
    releases: &[RubyDownloadVersion],
    minimum: &RubyDownloadVersion,
) -> Vec<RubyDownloadVersion> {
    releases
        .iter()
        .filter(|version| version_gte(version, minimum))
        .cloned()
        .collect()
}

fn urls_to_check(version: &RubyDownloadVersion) -> Vec<(String, Url)> {
    let matrix = build_matrix();
    let base_url = Url::parse(S3_BASE_URL).expect("valid base URL constant");
    matrix
        .iter()
        .map(|(base_image, arch)| {
            let tar_path = output_ruby_tar_path(Path::new(""), version, base_image, Some(arch));
            let mut url = base_url.clone();
            url.path_segments_mut()
                .expect("valid base URL")
                .extend(tar_path.iter().map(|s| s.to_string_lossy()));
            (format!("{base_image}/{arch}"), url)
        })
        .collect()
}

async fn s3_url_exists(url: Url) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
    let mut attempts = 0;
    loop {
        attempts += 1;
        match s3_url_exists_inner(url.clone()).await {
            Ok(val) => return Ok(val),
            Err(error) => {
                if attempts >= MAX_RETRY_ATTEMPTS {
                    return Err(error);
                }
                sleep(RETRY_DELAY).await;
            }
        }
    }
}

async fn s3_url_exists_inner(url: Url) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()?;
    let response = client.head(url.clone()).send().await?;
    match response.status() {
        status if status.is_success() => Ok(true),
        reqwest::StatusCode::NOT_FOUND | reqwest::StatusCode::FORBIDDEN => Ok(false),
        status => Err(format!("Unexpected status {status} checking {url}").into()),
    }
}

async fn check_version_on_s3(
    version: RubyDownloadVersion,
) -> Result<(RubyDownloadVersion, Vec<String>), Box<dyn std::error::Error + Send + Sync>> {
    let mut set = JoinSet::new();
    for (label, url) in urls_to_check(&version) {
        set.spawn(async move {
            let exists = s3_url_exists(url).await?;
            Ok::<_, Box<dyn std::error::Error + Send + Sync>>((label, exists))
        });
    }

    let mut missing = Vec::new();
    while let Some(result) = set.join_next().await {
        let (label, exists) = result??;
        if !exists {
            missing.push(label);
        }
    }

    Ok((version, missing))
}

async fn call(args: Args) -> Result<(), Box<dyn Error>> {
    print::h2("Checking for new Ruby releases");
    print::bullet(format!("Minimum version: {}", args.minimum_version));

    print::h2(format!("Fetching releases from {}", *RELEASES_URL));
    let releases = match fetch_releases(&RELEASES_URL).await {
        Ok(r) => r,
        Err(e) => {
            print::error(format!("Failed to fetch releases: {e}"));
            std::process::exit(1);
        }
    };
    print::bullet(format!("Found {} total releases", releases.len()));

    let versions_to_check = retain_releases_gte(&releases, &args.minimum_version);

    print::bullet(format!(
        "Checking {} versions on S3",
        versions_to_check.len()
    ));

    let mut set = JoinSet::new();
    for version in versions_to_check {
        set.spawn(check_version_on_s3(version));
    }

    let mut versions_to_build = Vec::new();
    while let Some(result) = set.join_next().await {
        match result? {
            Ok((version, missing)) if missing.is_empty() => {
                print::sub_bullet(format!("{version}: all binaries present"));
            }
            Ok((version, missing)) => {
                print::sub_bullet(format!(
                    "{version}: missing {} combo(s): {}",
                    missing.len(),
                    missing.join(", ")
                ));
                versions_to_build.push(version);
            }
            Err(e) => {
                print::warning(format!("Error checking version: {e}"));
            }
        }
    }

    fs::write(
        &args.output,
        &serde_json::to_string_pretty(&versions_to_build)?,
    )?;
    if versions_to_build.is_empty() {
        print::bullet("All checked versions are present on S3");
    } else {
        print::h2("Versions needing builds");
        for version in &versions_to_build {
            print::sub_bullet(format!("{version}"));
        }
    }
    Ok(())
}

#[tokio::main]
async fn main() {
    let args = Args::parse();
    match call(args).await {
        Ok(_) => print::bullet("Done"),
        Err(e) => {
            print::error(format!("Failed {e}"));
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_version_gte() {
        let min = RubyDownloadVersion::new("3.2.0").unwrap();
        assert!(version_gte(
            &RubyDownloadVersion::new("3.2.0").unwrap(),
            &min
        ));
        assert!(version_gte(
            &RubyDownloadVersion::new("3.3.7").unwrap(),
            &min
        ));
        assert!(version_gte(
            &RubyDownloadVersion::new("4.0.0").unwrap(),
            &min
        ));
        assert!(!version_gte(
            &RubyDownloadVersion::new("3.1.9").unwrap(),
            &min
        ));
        assert!(!version_gte(
            &RubyDownloadVersion::new("2.7.8").unwrap(),
            &min
        ));
    }

    #[test]
    fn test_version_gte_prerelease() {
        let min = RubyDownloadVersion::new("3.4.0").unwrap();
        assert!(version_gte(
            &RubyDownloadVersion::new("3.4.0-preview1").unwrap(),
            &min
        ));
        assert!(!version_gte(
            &RubyDownloadVersion::new("3.3.9").unwrap(),
            &min
        ));
    }

    #[test]
    fn test_retain_releases_gte() {
        let releases = vec![
            RubyDownloadVersion::new("3.4.1").unwrap(),
            RubyDownloadVersion::new("3.3.7").unwrap(),
            RubyDownloadVersion::new("3.2.0").unwrap(),
            RubyDownloadVersion::new("3.1.5").unwrap(),
            RubyDownloadVersion::new("2.7.8").unwrap(),
        ];
        let min = RubyDownloadVersion::new("3.2.0").unwrap();
        let filtered = retain_releases_gte(&releases, &min);
        let names: Vec<String> = filtered.iter().map(|v| v.to_string()).collect();
        assert_eq!(names, vec!["3.4.1", "3.3.7", "3.2.0"]);
    }
}
