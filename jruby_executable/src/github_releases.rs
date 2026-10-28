//! JRuby releases published on GitHub and their prebuilt binaries on S3.

use crate::JRubyVersion;
use serde::Deserialize;
use shared::github::{self, GitHubToken};
use shared::maybe_err::ResultVec;
use shared::{BuildTarget, parse_url, s3_url};
use std::future::Future;
use std::path::Path;
use url::Url;

pub static RELEASES_URL: std::sync::LazyLock<Url> = std::sync::LazyLock::new(|| {
    // per_page=100 is the GitHub releases API maximum page size.
    parse_url("https://api.github.com/repos/jruby/jruby/releases?per_page=100")
        .expect("internal literal should be a valid URL")
});

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
pub struct ReleasePageError {
    pub url: Url,
    pub source: GithubReleaseError,
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
pub enum GithubReleaseError {
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
    CannotParseJrubyVersion(#[from] crate::jruby_version::ParseError),
}

/// The S3 URL where the prebuilt binary for `version` would live on `target`
pub fn s3_url_to_check(
    version: &JRubyVersion,
    ruby_stdlib_version: &str,
    target: &BuildTarget,
) -> Url {
    let path = Path::new(target.base_image.name())
        .join(target.arch.to_string())
        .join(format!("ruby-{ruby_stdlib_version}-jruby-{version}.tgz"));
    s3_url(&path)
}

/// List every non-prerelease JRuby version from the GitHub releases API.
///
/// Partial results are kept: versions from pages fetched before a failure are returned alongside the error.
pub async fn fetch_releases(gh_token: &GitHubToken) -> ResultVec<JRubyVersion, ReleasePageError> {
    paginate_releases_accumulated(RELEASES_URL.clone(), |url| async move {
        let response = github::get_json_with_retry(&url, gh_token).await?;

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
}

#[cfg(test)]
mod tests {
    use super::*;
    use shared::build_matrix;

    #[test]
    fn test_s3_url_to_check() {
        let version = JRubyVersion::parse("9.4.7.0").unwrap();
        let urls = build_matrix()
            .iter()
            .map(|target| s3_url_to_check(&version, "3.1.4", target).to_string())
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
            parse_url("https://api.github.com/repos/jruby/jruby/releases?per_page=100").unwrap();

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
            parse_url("https://api.github.com/repos/jruby/jruby/releases?per_page=100").unwrap();
        let page2 =
            parse_url("https://api.github.com/repos/jruby/jruby/releases?per_page=100&page=2")
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
            parse_url("https://api.github.com/repos/jruby/jruby/releases?per_page=100").unwrap();

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
            parse_url("https://api.github.com/repos/jruby/jruby/releases?per_page=100").unwrap();

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
