//! Check for JRuby releases that aren't pushed to S3 yet
//!
//! ```term
//! $ cargo run --bin jruby_release_check -- --help
//! ```

use bullet_stream::global::print;
use clap::Parser;
use fs_err as fs;
use jruby_executable::{JRubyVersion, jruby_build_properties, jruby_version};
use libherokubuildpack::inventory::artifact::Arch;
use serde::Deserialize;
use shared::github::{self, GitHubToken};
use shared::maybe_err::ResultVec;
use shared::{BaseImage, S3_BASE_URL};
use shared::{build_matrix, s3_url_exists};
use std::{error::Error, future::Future, path::PathBuf};
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
    /// Required. Generate one locally with: `--gh-token=$(gh auth token)`
    #[arg(long = "gh-token", value_parser = |s: &str| -> Result<GitHubToken, String> {
        GitHubToken::try_from(s).map_err(|error| format!("{error}. Suggestion: generate one locally with: `--gh-token=$(gh auth token)`"))
    })]
    gh_token: GitHubToken,

    /// Minimum JRuby version to check (e.g. 9.4.7.0). All releases >= this version will be checked.
    #[arg(long = "minimum-version", required = true)]
    minimum_version: JRubyVersion,

    /// Path to write JSON output file containing versions that need builds
    #[arg(long = "output", required = true)]
    output: PathBuf,
}

/// A single entry from the GitHub releases listing API.
///
/// Only the fields needed to discover JRuby versions are deserialized; the rest
/// of the payload is ignored.
#[derive(Deserialize)]
struct RawGitHubRelease {
    /// The release's git tag (e.g. `"9.4.15.0"`), optionally prefixed with `v`.
    /// Parsed into a [`JRubyVersion`] after stripping the leading `v`.
    tag_name: String,
    /// Whether GitHub flagged this as a prerelease. Prereleases are filtered out
    /// so only stable versions are considered.
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
) -> ResultVec<JRubyVersion, ReleasePageError>
where
    F: FnMut(Url) -> Fut,
    Fut: Future<Output = Result<(Vec<RawGitHubRelease>, Option<Url>), GithubReleaseError>>,
{
    let mut next = Some(base_url);

    let mut results: Vec<Result<JRubyVersion, ReleasePageError>> = Vec::new();

    while let Some(url) = next {
        match fetch(url.clone()).await {
            Ok((releases, next_url)) => {
                for release in releases.into_iter().filter(|r| !r.prerelease) {
                    let tag = release
                        .tag_name
                        .strip_prefix('v')
                        .unwrap_or(&release.tag_name);

                    results.push(JRubyVersion::parse(tag).map_err(|error| ReleasePageError {
                        url: url.clone(),
                        source: GithubReleaseError::CannotParseJrubyVersion(error),
                    }))
                }
                next = next_url;
            }
            Err(source) => {
                results.push(Err(ReleasePageError { url, source }));
                break;
            }
        }
    }

    results.into()
}

#[derive(Debug, thiserror::Error)]
enum GithubReleaseError {
    #[error(transparent)]
    Http(#[from] reqwest::Error),

    #[error("could not parse pagination {0}")]
    Pagination(#[from] shared::github::GitHubHeaderError),

    #[error("could not parse releases response as JSON due to {error}. Body: {body}")]
    ReleaseResponseParse {
        body: String,
        error: serde_json::Error,
    },

    #[error(transparent)]
    CannotParseJrubyVersion(#[from] jruby_version::ParseError),
}

/// Keep only the releases at or above `minimum`, narrowing the full release
/// listing down to the versions worth checking on S3.
///
/// Ordering uses [`JRubyVersion`]'s field-wise comparison (major, then minor,
/// then patch, then extra), so `9.4.7.0` and anything newer is retained while
/// older versions are dropped. The input slice is left untouched; matching
/// versions are cloned into the returned vector.
fn retain_releases_gte(releases: &[JRubyVersion], minimum: &JRubyVersion) -> Vec<JRubyVersion> {
    releases
        .iter()
        .filter(|version| *version >= minimum)
        .cloned()
        .collect()
}

/// Build the set of S3 URLs where the prebuilt binary for `version` would live
fn s3_urls_to_check(
    version: &JRubyVersion,
    ruby_stdlib_version: &str,
) -> Vec<(Url, BaseImage, Arch)> {
    let base_url = Url::parse(S3_BASE_URL).expect("internal Url is parsable");

    build_matrix()
        .iter()
        .map(|(base_image, arch)| {
            let mut url = base_url.clone();
            url.path_segments_mut()
                .expect("valid base URL")
                .push(base_image.name())
                .push(&arch.to_string())
                .push(&format!("ruby-{ruby_stdlib_version}-jruby-{version}.tgz"));
            (url, base_image.clone(), *arch)
        })
        .collect()
}

/// Look up the Ruby standard-library version that the given JRuby `version`
/// implements, returning it paired with the version it belongs to.
async fn resolve_stdlib_version(
    version: JRubyVersion,
) -> Result<(JRubyVersion, String), Box<dyn Error + Send + Sync>> {
    let stdlib = jruby_build_properties(&version)
        .await
        .and_then(|props| props.ruby_stdlib_version())?;
    Ok((version, stdlib))
}

/// Contains list of found and missing binaries on S3 for given JRuby version
struct JRubyBinaries {
    version: JRubyVersion,
    #[allow(dead_code)]
    present: Vec<(BaseImage, Arch)>,
    missing: Vec<(BaseImage, Arch)>,
}

/// Check whether `version`'s prebuilt binary already exists on S3 for every
/// supported base image and supported architecture
async fn check_version_on_s3(
    version: JRubyVersion,
    ruby_stdlib_version: String,
) -> Result<JRubyBinaries, Box<dyn Error + Send + Sync>> {
    let mut set = JoinSet::new();
    for (url, image, arch) in s3_urls_to_check(&version, &ruby_stdlib_version) {
        set.spawn(async move {
            let exists = s3_url_exists(url).await?;
            Ok::<_, Box<dyn Error + Send + Sync>>((exists, image, arch))
        });
    }

    let mut missing = Vec::new();
    let mut present = Vec::new();
    while let Some(result) = set.join_next().await {
        let (exists, image, arch) = result??;
        if exists {
            present.push((image, arch))
        } else {
            missing.push((image, arch));
        }
    }

    Ok(JRubyBinaries {
        version,
        present,
        missing,
    })
}

/// Attaches human-facing stage context (e.g. "resolving stdlib version") to a
/// type-erased error while preserving its `source()` chain, so the integration
/// layer can keep "which phase failed" without falling back to a bare `String`.
#[derive(Debug, thiserror::Error)]
#[error("{context}: {source}")]
struct StageError {
    context: String,
    #[source]
    source: Box<dyn Error + Send + Sync>,
}

async fn call(args: Args) -> Result<(), Vec<Box<dyn Error>>> {
    print::h2("Checking for new JRuby releases");
    print::bullet(format!("Minimum version: {}", args.minimum_version));

    print::h2(format!("Fetching releases from {}", *RELEASES_URL));
    // Type erasure at the last responsible moment: upstream code stays strongly
    // typed for as long as it can, and only here -- where many unrelated failures
    // are integrated into one report -- do we collapse them to `dyn Error`. This is
    // erasure, not stringification: the boxed error still carries its source chain;
    // a `String` would throw that away. Text is produced only when we print/return.
    let mut errors: Vec<Box<dyn Error>> = Vec::new();
    let gh_token = &args.gh_token;
    let releases = paginate_releases_accumulated(RELEASES_URL.clone(), |url| async move {
        let response = github::get_with_auth_and_retry(&url, gh_token).await?;

        let releases = serde_json::from_str(&response.body).map_err(|error| {
            GithubReleaseError::ReleaseResponseParse {
                body: response.body.clone(),
                error,
            }
        })?;
        let next = github::GitHubPagination::from(response)?.next;
        Ok((releases, next))
    })
    .await
    .unwrap_drain_errs(&mut errors);

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
                }));
            }
            Err(join_err) => {
                print::warning(format!(
                    "Task panicked resolving stdlib version: {join_err}"
                ));
                errors.push(Box::new(StageError {
                    context: "task panicked resolving stdlib version".to_string(),
                    source: Box::new(join_err),
                }));
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
            Ok(Ok(JRubyBinaries {
                version,
                present: _,
                missing,
            })) if missing.is_empty() => {
                if errors.is_empty() {
                    print::sub_bullet(format!("{version}: all binaries present"));
                }
            }
            Ok(Ok(JRubyBinaries {
                version,
                present: _,
                missing,
            })) => {
                print::sub_bullet(format!(
                    "{version}: missing {} base image(s): {}",
                    missing.len(),
                    missing
                        .iter()
                        .map(|(image, arch)| format!("{}/{}", image.name(), arch))
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
                versions_to_build.push(version);
            }
            Ok(Err(e)) => {
                print::warning(format!("Error checking version: {e}"));
                errors.push(Box::new(StageError {
                    context: "checking S3 for version".to_string(),
                    source: e,
                }));
            }
            Err(join_err) => {
                print::warning(format!("Task panicked checking version: {join_err}"));
                errors.push(Box::new(StageError {
                    context: "task panicked checking S3 for version".to_string(),
                    source: Box::new(join_err),
                }));
            }
        }
    }

    fs::write(
        &args.output,
        &serde_json::to_string_pretty(&versions_to_build).map_err(|e| vec![e.into()])?,
    )
    .map_err(|e| vec![e.into()])?;

    if versions_to_build.is_empty() && errors.is_empty() {
        print::bullet("All checked versions are present on S3");
    } else {
        print::h2("Versions needing builds");
        for version in &versions_to_build {
            print::sub_bullet(format!("{version}"));
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

/// Formats a string into a single bullet point item
///
/// Multi line text correctly lines up with the text above it instead of with the dash.
fn bullet_point(s: impl AsRef<str>) -> String {
    let input = s.as_ref();
    if input.is_empty() {
        String::from("- ")
    } else {
        input
            .split_inclusive('\n')
            .enumerate()
            .map(|(line_index, line)| {
                let prefix = if line_index == 0 { "- " } else { "  " };
                prefix.to_string() + line
            })
            .collect()
    }
}

#[tokio::main]
async fn main() {
    let args = Args::parse();
    match call(args).await {
        Ok(()) => print::bullet("Done"),
        Err(errors) => {
            print::error(format!(
                "Failed! Errors:\n{}",
                errors
                    .iter()
                    .map(|error| bullet_point(error.to_string()))
                    .collect::<Vec<_>>()
                    .join("\n")
            ));
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bullet_point_formatting() {
        assert_eq!("- hello world", bullet_point("hello world"));
        assert_eq!("- ", bullet_point(""));
        assert_eq!(
            indoc::indoc! {"
                - hello
                  world
            "},
            bullet_point("hello\nworld\n")
        );
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
        let urls = s3_urls_to_check(&version, "3.1.4")
            .into_iter()
            .map(|(url, _, _)| url.to_string())
            .collect::<Vec<_>>();
        assert_eq!(urls.len(), 5);

        for expected in [
            "https://heroku-buildpack-ruby.s3.dualstack.us-east-1.amazonaws.com/heroku-22/amd64/ruby-3.1.4-jruby-9.4.7.0.tgz",
            "https://heroku-buildpack-ruby.s3.dualstack.us-east-1.amazonaws.com/heroku-24/amd64/ruby-3.1.4-jruby-9.4.7.0.tgz",
            "https://heroku-buildpack-ruby.s3.dualstack.us-east-1.amazonaws.com/heroku-24/arm64/ruby-3.1.4-jruby-9.4.7.0.tgz",
            "https://heroku-buildpack-ruby.s3.dualstack.us-east-1.amazonaws.com/heroku-26/amd64/ruby-3.1.4-jruby-9.4.7.0.tgz",
            "https://heroku-buildpack-ruby.s3.dualstack.us-east-1.amazonaws.com/heroku-26/arm64/ruby-3.1.4-jruby-9.4.7.0.tgz",
        ] {
            assert!(
                urls.iter().find(|url| *url == expected).is_some(),
                "expected `{expected}` in collection but it was not found:\n{urls:?}"
            );
        }
    }

    #[tokio::test]
    async fn paginate_strips_leading_v_from_tag() {
        let page1 =
            Url::parse("https://api.github.com/repos/jruby/jruby/releases?per_page=100").unwrap();

        let mut errors: Vec<ReleasePageError> = Vec::new();
        let versions = paginate_releases_accumulated(page1, move |_url| async move {
            Ok::<(Vec<RawGitHubRelease>, Option<Url>), GithubReleaseError>((
                vec![release("v9.4.7.0")],
                None,
            ))
        })
        .await
        .unwrap_drain_errs(&mut errors);

        let names: Vec<String> = versions.iter().map(|v| v.to_string()).collect();
        assert_eq!(names, vec!["9.4.7.0"]);
        assert!(errors.is_empty());
    }

    #[test]
    fn test_deserialize_github_release() {
        let json = r#"{"tag_name": "9.4.15.0", "prerelease": false}"#;
        let release: RawGitHubRelease = serde_json::from_str(json).unwrap();
        assert_eq!(release.tag_name, "9.4.15.0");
        assert!(!release.prerelease);
    }

    #[test]
    fn test_deserialize_github_release_prerelease() {
        let json = r#"{"tag_name": "9.5.0.0.pre1", "prerelease": true}"#;
        let release: RawGitHubRelease = serde_json::from_str(json).unwrap();
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

    fn release(tag: &str) -> RawGitHubRelease {
        RawGitHubRelease {
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

        let mut errors: Vec<ReleasePageError> = Vec::new();
        let versions = paginate_releases_accumulated(page1, move |url| {
            let page2 = page2.clone();
            async move {
                if url.as_str().contains("page=2") {
                    Err(parse_failure())
                } else {
                    Ok((vec![release("9.4.15.0"), release("9.4.14.0")], Some(page2)))
                }
            }
        })
        .await
        .unwrap_drain_errs(&mut errors);

        let names: Vec<String> = versions.iter().map(|v| v.to_string()).collect();
        assert_eq!(names, vec!["9.4.15.0", "9.4.14.0"]);
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].url, expected_failed);
    }

    #[tokio::test]
    async fn paginate_returns_no_error_when_all_pages_succeed() {
        let page1 =
            Url::parse("https://api.github.com/repos/jruby/jruby/releases?per_page=100").unwrap();

        let mut errors: Vec<ReleasePageError> = Vec::new();
        let versions = paginate_releases_accumulated(page1, move |_url| async move {
            Ok::<(Vec<RawGitHubRelease>, Option<Url>), GithubReleaseError>((
                vec![release("9.4.15.0")],
                None,
            ))
        })
        .await
        .unwrap_drain_errs(&mut errors);

        assert_eq!(versions.len(), 1);
        assert!(errors.is_empty());
    }

    #[tokio::test]
    async fn paginate_accumulates_version_parse_errors_and_keeps_collecting() {
        let page1 =
            Url::parse("https://api.github.com/repos/jruby/jruby/releases?per_page=100").unwrap();

        let mut errors: Vec<ReleasePageError> = Vec::new();
        let versions = paginate_releases_accumulated(page1, move |_url| async move {
            Ok::<(Vec<RawGitHubRelease>, Option<Url>), GithubReleaseError>((
                vec![release("9.4.15.0"), release("not-a-version")],
                None,
            ))
        })
        .await
        .unwrap_drain_errs(&mut errors);

        let names: Vec<String> = versions.iter().map(|v| v.to_string()).collect();
        assert_eq!(names, vec!["9.4.15.0"]);
        assert_eq!(errors.len(), 1);
        assert!(
            matches!(
                errors[0].source,
                GithubReleaseError::CannotParseJrubyVersion(_)
            ),
            "got: {:?}",
            errors[0]
        );
    }
}
