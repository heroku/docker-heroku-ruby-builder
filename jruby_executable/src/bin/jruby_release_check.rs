use bullet_stream::global::print;
use clap::Parser;
use fs_err as fs;
use jruby_executable::jruby_build_properties;
use serde::Deserialize;
use shared::github;
use shared::maybe_err::{MaybeErrors, MultiErrors, OkMaybe};
use shared::s3;
use shared::{S3_BASE_URL, base_images};
use std::{error::Error, fmt, future::Future, path::PathBuf};
use tokio::task::JoinSet;
use url::Url;

static RELEASES_URL: std::sync::LazyLock<Url> = std::sync::LazyLock::new(|| {
    // per_page=100 is the GitHub releases API maximum page size.
    Url::parse("https://api.github.com/repos/jruby/jruby/releases?per_page=100")
        .expect("valid releases URL constant")
});

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

/// A page of the releases listing failed to fetch after retries.
///
/// Pagination cannot continue past a failed page (the `next` link lives in the
/// failed response), so this is returned alongside whatever releases were
/// collected from earlier pages rather than discarding them.
#[derive(Debug, thiserror::Error)]
#[error("failed fetching releases page {url}: {source}")]
struct ReleasePageError {
    url: Url,
    source: GithubReleaseError,
}

/// Drive pagination over release pages, accumulating parsed versions and any
/// errors encountered along the way.
///
/// `fetch` is injected so the pagination/partial-success logic can be exercised
/// without real network access. Versions that fail to parse are collected as
/// errors rather than dropped, and on a page-fetch failure the versions gathered
/// so far are returned together with the error(s) (pagination cannot continue
/// past a failed page, since the `next` link lives in the failed response).
async fn paginate_releases_accumulated<F, Fut>(
    base_url: Url,
    mut fetch: F,
) -> OkMaybe<Vec<JRubyVersion>, MultiErrors<ReleasePageError>>
where
    F: FnMut(Url) -> Fut,
    Fut: Future<Output = Result<(Vec<GitHubRelease>, Option<Url>), GithubReleaseError>>,
{
    let mut errors = MaybeErrors::new();
    let mut versions = Vec::new();
    let mut next = Some(base_url);

    while let Some(url) = next {
        match fetch(url.clone()).await {
            Ok((releases, next_url)) => {
                for release in releases.into_iter().filter(|r| !r.prerelease) {
                    let tag = release
                        .tag_name
                        .strip_prefix('v')
                        .unwrap_or(&release.tag_name);
                    match JRubyVersion::parse(tag) {
                        Ok(version) => versions.push(version),
                        Err(error) => errors.push(ReleasePageError {
                            url: url.clone(),
                            source: GithubReleaseError::CannotParseJrubyVersion {
                                raw: tag.to_owned(),
                                error,
                            },
                        }),
                    }
                }
                next = next_url;
            }
            Err(source) => {
                errors.push(ReleasePageError { url, source });
                break;
            }
        }
    }

    errors.ok_maybe(versions)
}

async fn fetch_release_page(
    url: &Url,
    token: &str,
) -> Result<(Vec<GitHubRelease>, Option<Url>), GithubReleaseError> {
    let response = github::get_auth_with_retry(url, token).await?;
    let next = response.paginate_next()?;

    let releases = serde_json::from_str(&response.body).map_err(|error| {
        GithubReleaseError::ReleaseResponseParse {
            body: response.body,
            error,
        }
    })?;
    Ok((releases, next))
}

#[derive(Debug, thiserror::Error)]
enum GithubReleaseError {
    #[error(transparent)]
    Http(#[from] reqwest::Error),

    #[error("could not parse pagination {0}")]
    Pagination(#[from] shared::github::GithubHeaderError),

    #[error("could not parse releases response as JSON due to {error}. Body: {body}")]
    ReleaseResponseParse {
        body: String,
        error: serde_json::Error,
    },

    #[error("could not parse JRuby version: `{raw}` due to error {error}")]
    CannotParseJrubyVersion { raw: String, error: String },
}

fn retain_releases_gte(releases: &[JRubyVersion], minimum: &JRubyVersion) -> Vec<JRubyVersion> {
    releases
        .iter()
        .filter(|version| *version >= minimum)
        .cloned()
        .collect()
}

fn s3_urls_to_check(version: &JRubyVersion, ruby_stdlib_version: &str) -> Vec<(String, Url)> {
    let base_url = S3_BASE_URL.clone();
    base_images()
        .iter()
        .map(|base_image| {
            let tgz_name = format!("ruby-{ruby_stdlib_version}-jruby-{version}.tgz");
            let mut url = base_url.clone();
            url.path_segments_mut()
                .expect("valid base URL")
                .push(base_image.name())
                .push(&tgz_name);
            (base_image.name().to_string(), url)
        })
        .collect()
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
            let exists = s3::url_exists(url).await?;
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

/// Attaches human-facing stage context (e.g. "resolving stdlib version") to a
/// type-erased error while preserving its `source()` chain, so the integration
/// layer can keep "which phase failed" without falling back to a bare `String`.
#[derive(Debug, thiserror::Error)]
#[error("{context}")]
struct StageError {
    context: String,
    #[source]
    source: Box<dyn Error + Send + Sync>,
}

async fn call(args: ResolvedArgs) -> Result<(), Box<dyn Error>> {
    print::h2("Checking for new JRuby releases");
    print::bullet(format!("Minimum version: {}", args.minimum_version));

    print::h2(format!("Fetching releases from {}", *RELEASES_URL));
    // Type erasure at the last responsible moment: upstream code stays strongly
    // typed for as long as it can, and only here -- where many unrelated failures
    // are integrated into one report -- do we collapse them to `dyn Error`. This is
    // erasure, not stringification: the boxed error still carries its source chain;
    // a `String` would throw that away. Text is produced only when we print/return.
    let mut errors: MaybeErrors<Box<dyn Error + Send + Sync>> = MaybeErrors::new();
    let gh_token = &args.gh_token;
    let OkMaybe(releases, fetch_errors) =
        paginate_releases_accumulated(RELEASES_URL.clone(), |url| async move {
            fetch_release_page(&url, gh_token).await
        })
        .await;

    if let Some(fetch_errors) = fetch_errors {
        for failure in fetch_errors {
            errors.push(Box::new(failure) as Box<dyn Error + Send + Sync>);
        }
    }
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
        match result {
            Ok(Ok((version, stdlib))) => {
                print::sub_bullet(format!("{version} -> Ruby stdlib {stdlib}"));
                resolved.push((version, stdlib));
            }
            Ok(Err(e)) => {
                print::warning(format!("Error resolving stdlib version: {e}"));
                errors.push(Box::new(StageError {
                    context: "resolving stdlib version".to_string(),
                    source: e,
                }) as Box<dyn Error + Send + Sync>);
            }
            Err(join_err) => {
                print::warning(format!(
                    "Task panicked resolving stdlib version: {join_err}"
                ));
                errors.push(Box::new(StageError {
                    context: "task panicked resolving stdlib version".to_string(),
                    source: Box::new(join_err),
                }) as Box<dyn Error + Send + Sync>);
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
        match result {
            Ok(Ok((version, missing))) if missing.is_empty() => {
                print::sub_bullet(format!("{version}: all binaries present"));
            }
            Ok(Ok((version, missing))) => {
                print::sub_bullet(format!(
                    "{version}: missing {} base image(s): {}",
                    missing.len(),
                    missing.join(", ")
                ));
                versions_to_build.push(version);
            }
            Ok(Err(e)) => {
                print::warning(format!("Error checking version: {e}"));
                errors.push(Box::new(StageError {
                    context: "checking S3 for version".to_string(),
                    source: e,
                }) as Box<dyn Error + Send + Sync>);
            }
            Err(join_err) => {
                print::warning(format!("Task panicked checking version: {join_err}"));
                errors.push(Box::new(StageError {
                    context: "task panicked checking S3 for version".to_string(),
                    source: Box::new(join_err),
                }) as Box<dyn Error + Send + Sync>);
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

    if !errors.is_empty() {
        print::error(format!("{} check(s) failed", errors.len()));
        for failure in &errors {
            print::sub_bullet(failure.to_string());
        }
        return Err(format!("{} check(s) failed", errors.len()).into());
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

    #[test]
    fn test_releases_url_sets_per_page() {
        assert!(
            RELEASES_URL.as_str().contains("per_page=100"),
            "got: {}",
            RELEASES_URL.as_str()
        );
    }

    fn release(tag: &str) -> GitHubRelease {
        GitHubRelease {
            tag_name: tag.to_string(),
            prerelease: false,
        }
    }

    fn parse_failure() -> GithubReleaseError {
        let body = String::from("not json");
        serde_json::from_str::<i32>(&body)
            .map_err(|error| GithubReleaseError::ReleaseResponseParse { body, error })
            .unwrap_err()
    }

    #[tokio::test]
    async fn paginate_keeps_releases_collected_before_a_failed_page() {
        let page1 =
            Url::parse("https://api.github.com/repos/jruby/jruby/releases?per_page=100").unwrap();
        let page2 =
            Url::parse("https://api.github.com/repos/jruby/jruby/releases?per_page=100&page=2")
                .unwrap();
        let expected_failed = page2.clone();

        let OkMaybe(versions, errors) = paginate_releases_accumulated(page1, move |url| {
            let page2 = page2.clone();
            async move {
                if url.as_str().contains("page=2") {
                    Err(parse_failure())
                } else {
                    Ok((vec![release("9.4.15.0"), release("9.4.14.0")], Some(page2)))
                }
            }
        })
        .await;

        let names: Vec<String> = versions.iter().map(|v| v.to_string()).collect();
        assert_eq!(names, vec!["9.4.15.0", "9.4.14.0"]);
        let errors = errors.expect("expected the failed page to be reported");
        assert_eq!(errors.len().get(), 1);
        assert_eq!(errors.iter().next().unwrap().url, expected_failed);
    }

    #[tokio::test]
    async fn paginate_returns_no_error_when_all_pages_succeed() {
        let page1 =
            Url::parse("https://api.github.com/repos/jruby/jruby/releases?per_page=100").unwrap();

        let OkMaybe(versions, errors) =
            paginate_releases_accumulated(page1, move |_url| async move {
                Ok::<(Vec<GitHubRelease>, Option<Url>), GithubReleaseError>((
                    vec![release("9.4.15.0")],
                    None,
                ))
            })
            .await;

        assert_eq!(versions.len(), 1);
        assert!(errors.is_none());
    }

    #[tokio::test]
    async fn paginate_accumulates_version_parse_errors_and_keeps_collecting() {
        let page1 =
            Url::parse("https://api.github.com/repos/jruby/jruby/releases?per_page=100").unwrap();

        let OkMaybe(versions, errors) =
            paginate_releases_accumulated(page1, move |_url| async move {
                Ok::<(Vec<GitHubRelease>, Option<Url>), GithubReleaseError>((
                    vec![release("9.4.15.0"), release("not-a-version")],
                    None,
                ))
            })
            .await;

        let names: Vec<String> = versions.iter().map(|v| v.to_string()).collect();
        assert_eq!(names, vec!["9.4.15.0"]);
        let errors = errors.expect("expected the unparseable tag to be reported");
        assert_eq!(errors.len().get(), 1);
        assert!(
            matches!(
                errors.iter().next().unwrap().source,
                GithubReleaseError::CannotParseJrubyVersion { .. }
            ),
            "got: {:?}",
            errors.iter().next().unwrap()
        );
    }
}
