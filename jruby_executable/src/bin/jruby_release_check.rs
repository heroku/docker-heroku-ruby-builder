use bullet_stream::global::print;
use clap::Parser;
use fs_err as fs;
use jruby_executable::jruby_build_properties;
use reqwest::Url;
use serde::Deserialize;
use shared::S3_BASE_URL;
use std::{error::Error, fmt, path::PathBuf, time::Duration};
use tokio::task::JoinSet;
use tokio::time::sleep;

static RELEASES_URL: std::sync::LazyLock<Url> = std::sync::LazyLock::new(|| {
    Url::parse("https://api.github.com/repos/jruby/jruby/releases?per_page=100")
        .expect("valid releases URL constant")
});

const MAX_RETRY_ATTEMPTS: u8 = 3;
const RETRY_DELAY: Duration = Duration::from_secs(1);

static JRUBY_BASE_IMAGES: &[&str] = &["heroku-22", "heroku-24", "heroku-26"];

#[derive(Parser, Debug)]
#[command(about = "Check for JRuby releases missing from Heroku S3")]
struct Args {
    /// GitHub API token used to authenticate release lookups.
    ///
    /// Required. Generate one locally with: --gh-token=$(gh auth token)
    #[arg(long = "gh-token")]
    gh_token: Option<String>,

    /// Minimum JRuby version to check (e.g. 9.4.7.0). All releases >= this version will be checked.
    #[arg(long = "minimum-version", required = true)]
    minimum_version: JRubyVersion,

    /// Path to write JSON output file containing versions that need builds
    #[arg(long = "output", required = true)]
    output: PathBuf,
}

/// Validated arguments: every field is guaranteed usable.
#[derive(Debug)]
struct ResolvedArgs {
    gh_token: String,
    minimum_version: JRubyVersion,
    output: PathBuf,
}

impl TryFrom<Args> for ResolvedArgs {
    type Error = &'static str;

    fn try_from(args: Args) -> Result<Self, Self::Error> {
        let gh_token = match args.gh_token {
            Some(token) if !token.trim().is_empty() => token.trim().to_owned(),
            _ => {
                return Err(
                    "A GitHub API token is required. The GitHub Releases API returns \
                            HTTP 403 for unauthenticated requests once rate-limited.\n\n\
                            Pass one explicitly:\n    --gh-token=$(gh auth token)",
                );
            }
        };
        Ok(ResolvedArgs {
            gh_token,
            minimum_version: args.minimum_version,
            output: args.output,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct JRubyVersion {
    major: u32,
    minor: u32,
    patch: u32,
    extra: u32,
}

impl JRubyVersion {
    fn parse(s: &str) -> Result<Self, String> {
        let parts: Vec<&str> = s.split('.').collect();
        if parts.len() != 4 {
            return Err(format!(
                "Invalid JRuby version '{s}': expected 4 parts (X.Y.Z.W)"
            ));
        }
        let major = parts[0]
            .parse()
            .map_err(|_| format!("Invalid major version in '{s}'"))?;
        let minor = parts[1]
            .parse()
            .map_err(|_| format!("Invalid minor version in '{s}'"))?;
        let patch = parts[2]
            .parse()
            .map_err(|_| format!("Invalid patch version in '{s}'"))?;
        let extra = parts[3]
            .parse()
            .map_err(|_| format!("Invalid extra version in '{s}'"))?;
        Ok(Self {
            major,
            minor,
            patch,
            extra,
        })
    }
}

impl fmt::Display for JRubyVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}.{}.{}.{}",
            self.major, self.minor, self.patch, self.extra
        )
    }
}

impl serde::Serialize for JRubyVersion {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

impl std::str::FromStr for JRubyVersion {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

#[derive(Deserialize)]
struct GitHubRelease {
    tag_name: String,
    prerelease: bool,
}

async fn fetch_releases(url: &Url, token: &str) -> Result<Vec<JRubyVersion>, Box<dyn Error>> {
    let mut attempts = 0;
    loop {
        attempts += 1;
        match fetch_releases_inner(url, token).await {
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

async fn fetch_releases_inner(url: &Url, token: &str) -> Result<Vec<JRubyVersion>, Box<dyn Error>> {
    let request = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .user_agent("heroku-ruby-builder")
        .build()?
        .get(url.clone())
        .bearer_auth(token);

    let body = request.send().await?.error_for_status()?.text().await?;

    let releases: Vec<GitHubRelease> = serde_json::from_str(&body)?;

    let versions = releases
        .into_iter()
        .filter(|r| !r.prerelease)
        .filter_map(|r| {
            let tag = r.tag_name.strip_prefix('v').unwrap_or(&r.tag_name);
            JRubyVersion::parse(tag).ok()
        })
        .collect();

    Ok(versions)
}

fn retain_releases_gte(releases: &[JRubyVersion], minimum: &JRubyVersion) -> Vec<JRubyVersion> {
    releases
        .iter()
        .filter(|version| *version >= minimum)
        .cloned()
        .collect()
}

fn s3_urls_to_check(version: &JRubyVersion, ruby_stdlib_version: &str) -> Vec<(String, Url)> {
    let base_url = Url::parse(S3_BASE_URL).expect("valid base URL constant");
    JRUBY_BASE_IMAGES
        .iter()
        .map(|base_image| {
            let tgz_name = format!("ruby-{ruby_stdlib_version}-jruby-{version}.tgz");
            let mut url = base_url.clone();
            url.path_segments_mut()
                .expect("valid base URL")
                .push(base_image)
                .push(&tgz_name);
            ((*base_image).to_string(), url)
        })
        .collect()
}

async fn s3_url_exists(url: Url) -> Result<bool, Box<dyn Error + Send + Sync>> {
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

async fn s3_url_exists_inner(url: Url) -> Result<bool, Box<dyn Error + Send + Sync>> {
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

async fn resolve_stdlib_version(
    version: JRubyVersion,
) -> Result<(JRubyVersion, String), Box<dyn Error + Send + Sync>> {
    let version_str = version.to_string();
    let stdlib = tokio::task::spawn_blocking(move || {
        jruby_build_properties(&version_str).and_then(|props| props.ruby_stdlib_version())
    })
    .await??;
    Ok((version, stdlib))
}

async fn check_version_on_s3(
    version: JRubyVersion,
    ruby_stdlib_version: String,
) -> Result<(JRubyVersion, Vec<String>), Box<dyn Error + Send + Sync>> {
    let mut set = JoinSet::new();
    for (label, url) in s3_urls_to_check(&version, &ruby_stdlib_version) {
        set.spawn(async move {
            let exists = s3_url_exists(url).await?;
            Ok::<_, Box<dyn Error + Send + Sync>>((label, exists))
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

async fn call(args: ResolvedArgs) -> Result<(), Box<dyn Error>> {
    print::h2("Checking for new JRuby releases");
    print::bullet(format!("Minimum version: {}", args.minimum_version));

    print::h2(format!("Fetching releases from {}", *RELEASES_URL));
    let releases = match fetch_releases(&RELEASES_URL, &args.gh_token).await {
        Ok(r) => r,
        Err(e) => {
            print::error(format!("Failed to fetch releases: {e}"));
            std::process::exit(1);
        }
    };
    print::bullet(format!("Found {} non-prerelease versions", releases.len()));

    let versions_to_check = retain_releases_gte(&releases, &args.minimum_version);
    print::bullet(format!(
        "Checking {} versions >= {}",
        versions_to_check.len(),
        args.minimum_version
    ));

    print::bullet("Ruby stdlib versions");
    let mut stdlib_set = JoinSet::new();
    for version in versions_to_check {
        stdlib_set.spawn(resolve_stdlib_version(version));
    }

    let mut resolved = Vec::new();
    while let Some(result) = stdlib_set.join_next().await {
        match result? {
            Ok((version, stdlib)) => {
                print::sub_bullet(format!("{version} -> Ruby stdlib {stdlib}"));
                resolved.push((version, stdlib));
            }
            Err(e) => {
                print::warning(format!("Error resolving stdlib version: {e}"));
            }
        }
    }

    print::bullet("Check S3 for missing binaries");
    let mut s3_set = JoinSet::new();
    for (version, stdlib) in resolved {
        s3_set.spawn(check_version_on_s3(version, stdlib));
    }

    let mut versions_to_build = Vec::new();
    while let Some(result) = s3_set.join_next().await {
        match result? {
            Ok((version, missing)) if missing.is_empty() => {
                print::sub_bullet(format!("{version}: all binaries present"));
            }
            Ok((version, missing)) => {
                print::sub_bullet(format!(
                    "{version}: missing {} base image(s): {}",
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
    let resolved: ResolvedArgs = match args.try_into() {
        Ok(resolved) => resolved,
        Err(message) => {
            print::error(message);
            std::process::exit(1);
        }
    };
    match call(resolved).await {
        Ok(()) => print::bullet("Done"),
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
    fn missing_gh_token_message_names_the_flag() {
        let args = Args {
            gh_token: None,
            minimum_version: JRubyVersion::parse("9.4.7.0").unwrap(),
            output: PathBuf::from("versions.json"),
        };
        let message = ResolvedArgs::try_from(args).unwrap_err();
        assert!(
            message.contains("--gh-token=$(gh auth token)"),
            "got: {message}"
        );
    }

    #[test]
    fn empty_gh_token_message_names_the_flag() {
        let args = Args {
            gh_token: Some("   ".to_owned()),
            minimum_version: JRubyVersion::parse("9.4.7.0").unwrap(),
            output: PathBuf::from("versions.json"),
        };
        let message = ResolvedArgs::try_from(args).unwrap_err();
        assert!(
            message.contains("--gh-token=$(gh auth token)"),
            "got: {message}"
        );
    }

    #[test]
    fn present_gh_token_resolves() {
        let args = Args {
            gh_token: Some("abc".to_owned()),
            minimum_version: JRubyVersion::parse("9.4.7.0").unwrap(),
            output: PathBuf::from("versions.json"),
        };
        let resolved = ResolvedArgs::try_from(args).unwrap();
        assert_eq!(resolved.gh_token, "abc");
    }

    #[test]
    fn test_parse_valid_version() {
        let v = JRubyVersion::parse("9.4.7.0").unwrap();
        assert_eq!(v.major, 9);
        assert_eq!(v.minor, 4);
        assert_eq!(v.patch, 7);
        assert_eq!(v.extra, 0);
    }

    #[test]
    fn test_parse_invalid_versions() {
        assert!(JRubyVersion::parse("9.4.7").is_err());
        assert!(JRubyVersion::parse("9.4").is_err());
        assert!(JRubyVersion::parse("9").is_err());
        assert!(JRubyVersion::parse("").is_err());
        assert!(JRubyVersion::parse("9.4.7.0.1").is_err());
        assert!(JRubyVersion::parse("a.b.c.d").is_err());
    }

    #[test]
    fn test_version_display() {
        let v = JRubyVersion::parse("9.4.7.0").unwrap();
        assert_eq!(v.to_string(), "9.4.7.0");

        let v = JRubyVersion::parse("10.1.0.0").unwrap();
        assert_eq!(v.to_string(), "10.1.0.0");
    }

    #[test]
    fn test_version_ordering() {
        let v940 = JRubyVersion::parse("9.4.0.0").unwrap();
        let v947 = JRubyVersion::parse("9.4.7.0").unwrap();
        let v9415 = JRubyVersion::parse("9.4.15.0").unwrap();
        let v1010 = JRubyVersion::parse("10.1.0.0").unwrap();

        assert!(v947 > v940);
        assert!(v9415 > v947);
        assert!(v1010 > v9415);
        assert!(v940 < v947);
        assert_eq!(v947, JRubyVersion::parse("9.4.7.0").unwrap());
    }

    #[test]
    fn test_retain_releases_gte() {
        let releases = vec![
            JRubyVersion::parse("10.1.0.0").unwrap(),
            JRubyVersion::parse("9.4.15.0").unwrap(),
            JRubyVersion::parse("9.4.7.0").unwrap(),
            JRubyVersion::parse("9.4.0.0").unwrap(),
            JRubyVersion::parse("9.3.0.0").unwrap(),
        ];
        let min = JRubyVersion::parse("9.4.7.0").unwrap();
        let filtered = retain_releases_gte(&releases, &min);
        let names: Vec<String> = filtered.iter().map(|v| v.to_string()).collect();
        assert_eq!(names, vec!["10.1.0.0", "9.4.15.0", "9.4.7.0"]);
    }

    #[test]
    fn test_s3_urls_to_check() {
        let version = JRubyVersion::parse("9.4.7.0").unwrap();
        let urls = s3_urls_to_check(&version, "3.1.4");
        assert_eq!(urls.len(), 3);

        let (label, url) = &urls[0];
        assert_eq!(label, "heroku-22");
        assert_eq!(
            url.as_str(),
            "https://heroku-buildpack-ruby.s3.dualstack.us-east-1.amazonaws.com/heroku-22/ruby-3.1.4-jruby-9.4.7.0.tgz"
        );

        let (label, url) = &urls[1];
        assert_eq!(label, "heroku-24");
        assert_eq!(
            url.as_str(),
            "https://heroku-buildpack-ruby.s3.dualstack.us-east-1.amazonaws.com/heroku-24/ruby-3.1.4-jruby-9.4.7.0.tgz"
        );

        let (label, url) = &urls[2];
        assert_eq!(label, "heroku-26");
        assert_eq!(
            url.as_str(),
            "https://heroku-buildpack-ruby.s3.dualstack.us-east-1.amazonaws.com/heroku-26/ruby-3.1.4-jruby-9.4.7.0.tgz"
        );
    }

    #[test]
    fn test_version_from_str() {
        let v: JRubyVersion = "9.4.7.0".parse().unwrap();
        assert_eq!(v.to_string(), "9.4.7.0");
    }

    #[test]
    fn test_strip_v_prefix() {
        let tag = "v9.4.7.0";
        let stripped = tag.strip_prefix('v').unwrap_or(tag);
        assert_eq!(
            JRubyVersion::parse(stripped).unwrap().to_string(),
            "9.4.7.0"
        );
    }

    #[test]
    fn test_version_serialize_json() {
        let v = JRubyVersion::parse("9.4.7.0").unwrap();
        let json = serde_json::to_string(&v).unwrap();
        assert_eq!(json, "\"9.4.7.0\"");

        let versions = vec![
            JRubyVersion::parse("10.1.0.0").unwrap(),
            JRubyVersion::parse("9.4.15.0").unwrap(),
        ];
        let json = serde_json::to_string_pretty(&versions).unwrap();
        assert_eq!(json, "[\n  \"10.1.0.0\",\n  \"9.4.15.0\"\n]");
    }

    #[test]
    fn test_deserialize_github_release() {
        let json = r#"{"tag_name": "9.4.15.0", "prerelease": false}"#;
        let release: GitHubRelease = serde_json::from_str(json).unwrap();
        assert_eq!(release.tag_name, "9.4.15.0");
        assert!(!release.prerelease);
    }

    #[test]
    fn test_deserialize_github_release_prerelease() {
        let json = r#"{"tag_name": "9.5.0.0.pre1", "prerelease": true}"#;
        let release: GitHubRelease = serde_json::from_str(json).unwrap();
        assert!(release.prerelease);
    }
}
