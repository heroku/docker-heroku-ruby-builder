//! Logic and types for working with GitHub API

use crate::with_retries;
use reqwest::Url;
use reqwest::header::{HeaderMap, LINK, ToStrError};
use std::fmt;
use std::time::Duration;
use winnow::{
    Parser, Result,
    ascii::space0,
    combinator::{alt, delimited, preceded, repeat, separated},
    token::{take_until, take_while},
};

/// A GitHub API token used to authenticate requests.
///
/// Wrapping the raw secret in a dedicated type keeps it from being confused with
/// the many other `&str`/`String` arguments these APIs thread around (URLs,
/// response bodies, tags, ...), where a transposed argument would compile but
/// send the wrong value. Its [`Debug`](fmt::Debug) impl redacts the value so the
/// token does not leak into logs, panic messages, or the derived `Debug` output
/// of any struct that holds one.
///
/// Use [`as_str`](GitHubToken::as_str) at the point of use to obtain the secret.
///
/// # Intentionally not implemented
///
/// `Display`, `ToString`, and `Deref`/`AsRef<str>` are deliberately **not**
/// implemented, and that omission is load-bearing -- please do not add them:
///
/// - `Display`/`ToString` would re-leak the secret through `{}`, `format!`,
///   `.to_string()`, and error messages -- the very paths the redacted `Debug`
///   exists to close. Omitting `Display` also makes `.to_string()` a compile
///   error, forcing the explicit `as_str().to_owned()`.
/// - `Deref<Target = str>` would enable deref coercion, silently turning a
///   `&GitHubToken` back into a `&str` anywhere one is expected and reopening
///   the argument-confusion hole this newtype closes.
///
/// Every path that exposes the raw secret should be the deliberate,
/// [`as_str`](GitHubToken::as_str) call.
#[derive(Clone, PartialEq, Eq)]
pub struct GitHubToken(String);

impl GitHubToken {
    /// Borrow the underlying secret, e.g. to set an `Authorization` header.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn try_from(input: &str) -> Option<Self> {
        let token = input.trim();
        if token.is_empty() {
            None
        } else {
            Some(GitHubToken(token.to_owned()))
        }
    }
}

impl fmt::Debug for GitHubToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("GitHubToken").field(&"[REDACTED]").finish()
    }
}

/// Performs an authenticated `GET` request and returns the response headers and body.
///
/// The request sends a `Bearer` token via the `Authorization` header and uses a
/// 30-second timeout. The entire fetch (connection, HTTP status check, and full
/// body download) is wrapped in [`with_retries`], so a failure while streaming
/// the body is retried rather than only connection setup and the status line.
///
/// The response headers are cloned out and returned to the caller so that pagination
/// parsing happens outside the retry closure, ensuring deterministic parse errors
/// will not be retried along with network requests.
///
/// # Errors
///
/// Returns a [`reqwest::Error`] if the client cannot be built, the request fails
/// after exhausting retries, the response status is not successful (see
/// [`reqwest::Response::error_for_status`]), or the body cannot be read.
///
/// # Examples
///
/// ```no_run
/// use shared::github::{self, GitHubToken};
///
/// # use reqwest::Url;
/// # async fn run() -> Result<(), reqwest::Error> {
/// let url = Url::parse("https://api.github.com/repos/jruby/jruby/releases").unwrap();
/// let token = GitHubToken::try_from("gh_token").unwrap();
/// let response = github::get_with_auth_and_retry(&url, &token).await?;
/// # let _ = response;
/// # Ok(())
/// # }
/// ```
pub async fn get_with_auth_and_retry(
    url: &Url,
    token: &GitHubToken,
) -> Result<GitHubResponse, reqwest::Error> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .user_agent("heroku-ruby-builder")
        .build()?;

    let (headers, body) = with_retries(|| async {
        let response = client
            .get(url.clone())
            .bearer_auth(token.as_str())
            .send()
            .await?
            .error_for_status()?;

        let headers = response.headers().clone();
        let body = response.text().await?;
        Ok::<_, reqwest::Error>((headers, body))
    })
    .await?;

    Ok(GitHubResponse { headers, body })
}

/// Represents a response from GitHub
///
/// Does what it says on the tin
#[derive(Debug, PartialEq, Clone)]
pub struct GitHubResponse {
    pub headers: HeaderMap,
    pub body: String,
}

/// Represents github pagination from a given [`GitHubResponse`]
///
/// Does what it says on the tin
#[derive(Debug, PartialEq, Clone)]
pub struct GitHubPagination {
    pub first: Option<Url>,
    pub prev: Option<Url>,
    pub next: Option<Url>,
    pub last: Option<Url>,
}

impl GitHubPagination {
    pub fn from(response: GitHubResponse) -> Result<GitHubPagination, GitHubHeaderError> {
        let mut pagination = GitHubPagination {
            first: None,
            prev: None,
            next: None,
            last: None,
        };

        for link in pagination_links(&response.headers)? {
            match link {
                PageLink::First(url) => pagination.first.insert(url),
                PageLink::Prev(url) => pagination.prev.insert(url),
                PageLink::Next(url) => pagination.next.insert(url),
                PageLink::Last(url) => pagination.last.insert(url),
            };
        }

        Ok(pagination)
    }
}

/// Errors when parsing a github header for pagination links
#[derive(thiserror::Error, Debug)]
pub enum GitHubHeaderError {
    #[error("Link header contains non-ASCII characters: {0}")]
    NonAsciiChars(ToStrError),

    #[error("Malformed Link header `{header}`: {message}")]
    Malformed { header: String, message: String },

    #[error("Cannot parse URL `{raw}` from Link header `{header}`: {source}")]
    CannotParseUrl {
        raw: String,
        header: String,
        source: url::ParseError,
    },
}

/// A single GitHub pagination link, identified by its `rel` value.
///
/// GitHub's REST API only ever emits these four relations, each pointing at a
/// page of results. Entries with any other `rel` are ignored.
#[derive(Debug, PartialEq, Eq)]
enum PageLink {
    First(Url),
    Prev(Url),
    Next(Url),
    Last(Url),
}

/// Parse every recognized pagination link from a GitHub `Link` header.
///
/// Returns an empty `Vec` when the header is absent (GitHub omits it entirely
/// when all results fit on one page). Entries whose `rel` is not one of
/// GitHub's four documented values are skipped. Every recognized entry's URL is
/// parsed, so a malformed URL anywhere in the header surfaces as an error.
fn pagination_links(headers: &HeaderMap) -> Result<Vec<PageLink>, GitHubHeaderError> {
    let Some(value) = headers.get(LINK) else {
        return Ok(Vec::new());
    };
    let header = value.to_str().map_err(GitHubHeaderError::NonAsciiChars)?;

    let entries =
        link_header
            .parse(header.trim())
            .map_err(|error| GitHubHeaderError::Malformed {
                header: header.to_owned(),
                message: error.to_string(),
            })?;

    let mut links = Vec::new();
    for entry in entries {
        if let Some(link) = entry.into_page_link(header)? {
            links.push(link);
        }
    }
    Ok(links)
}

/// A single comma-separated entry of a `Link` header: a target URL plus its
/// trailing `;`-separated parameters (e.g. `rel`, `type`).
#[derive(Debug, PartialEq, Eq)]
struct LinkEntry<'a> {
    url: &'a str,
    params: Vec<(&'a str, &'a str)>,
}

impl LinkEntry<'_> {
    /// Map a raw parsed entry onto a typed [`PageLink`].
    ///
    /// `Ok(None)` means the entry carries no `rel`, or a `rel` GitHub never
    /// emits for pagination; both are silently skipped rather than treated as
    /// errors. `header` is only used to enrich `GithubHeaderError::CannotParseUrl`.
    fn into_page_link(self, header: &str) -> Result<Option<PageLink>, GitHubHeaderError> {
        let Some((_, rel)) = self.params.iter().find(|(name, _)| *name == "rel") else {
            return Ok(None);
        };
        let variant = match *rel {
            "first" => PageLink::First,
            "prev" => PageLink::Prev,
            "next" => PageLink::Next,
            "last" => PageLink::Last,
            _ => return Ok(None),
        };
        let url = Url::parse(self.url).map_err(|source| GitHubHeaderError::CannotParseUrl {
            raw: self.url.to_owned(),
            header: header.to_owned(),
            source,
        })?;
        Ok(Some(variant(url)))
    }
}

/// `link-header = link-entry *( "," link-entry )`
fn link_header<'a>(input: &mut &'a str) -> Result<Vec<LinkEntry<'a>>> {
    separated(1.., link_entry, (space0, ',', space0)).parse_next(input)
}

/// `link-entry = "<" URI ">" *( ";" param )`
fn link_entry<'a>(input: &mut &'a str) -> Result<LinkEntry<'a>> {
    let url = delimited('<', take_until(1.., '>'), '>').parse_next(input)?;
    let params = repeat(0.., preceded((space0, ';', space0), param)).parse_next(input)?;
    Ok(LinkEntry { url, params })
}

/// `param = token "=" ( quoted-string / token )`
fn param<'a>(input: &mut &'a str) -> Result<(&'a str, &'a str)> {
    let name = take_while(1.., |c: char| {
        c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')
    })
    .parse_next(input)?;
    let _ = (space0, '=', space0).parse_next(input)?;
    let value = alt((quoted_string, token)).parse_next(input)?;
    Ok((name, value))
}

/// `quoted-string = DQUOTE *( <any char except DQUOTE> ) DQUOTE`
fn quoted_string<'a>(input: &mut &'a str) -> Result<&'a str> {
    delimited('"', take_until(0.., '"'), '"').parse_next(input)
}

/// `token = 1*( <any char except "," ";" or whitespace> )`
fn token<'a>(input: &mut &'a str) -> Result<&'a str> {
    take_while(1.., |c: char| !matches!(c, ',' | ';') && !c.is_whitespace()).parse_next(input)
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::header::HeaderValue;

    fn headers_with_link(value: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(LINK, HeaderValue::from_str(value).unwrap());
        headers
    }

    fn next_from_headers(headers: HeaderMap) -> Option<Url> {
        let response = GitHubResponse {
            headers,
            body: String::new(),
        };
        GitHubPagination::from(response).unwrap().next
    }

    fn next_from_link(value: &str) -> Option<Url> {
        next_from_headers(headers_with_link(value))
    }

    #[test]
    fn debug_redacts_token_secret() {
        let token = GitHubToken::try_from("supersecret").unwrap();
        let debug = format!("{token:?}");
        assert!(
            !debug.contains("supersecret"),
            "GitHubToken Debug output leaked the secret: {debug}"
        );
    }

    #[test]
    fn finds_next_among_multiple_entries() {
        let header = r#"<https://api.github.com/repos/jruby/jruby/releases?per_page=100&page=2>; rel="next", <https://api.github.com/repos/jruby/jruby/releases?per_page=100&page=5>; rel="last""#;
        let next = next_from_link(header);
        assert_eq!(
            next.map(|url| url.to_string()),
            Some(
                "https://api.github.com/repos/jruby/jruby/releases?per_page=100&page=2".to_owned()
            )
        );
    }

    #[test]
    fn next_can_appear_after_prev() {
        let header = r#"<https://api.github.com/x?page=1>; rel="prev", <https://api.github.com/x?page=3>; rel="next""#;
        let next = next_from_link(header);
        assert_eq!(
            next.map(|url| url.to_string()),
            Some("https://api.github.com/x?page=3".to_owned())
        );
    }

    #[test]
    fn last_page_has_no_next() {
        let header = r#"<https://api.github.com/x?page=1>; rel="prev", <https://api.github.com/x?page=1>; rel="first""#;
        assert_eq!(next_from_link(header), None);
    }

    #[test]
    fn missing_header_is_end_of_pagination() {
        assert_eq!(next_from_headers(HeaderMap::new()), None);
    }

    #[test]
    fn parses_all_four_relations_as_enum() {
        let header = r#"<https://api.github.com/x?page=1>; rel="prev", <https://api.github.com/x?page=3>; rel="next", <https://api.github.com/x?page=1>; rel="first", <https://api.github.com/x?page=9>; rel="last""#;
        let links = pagination_links(&headers_with_link(header)).unwrap();
        assert_eq!(
            links,
            vec![
                PageLink::Prev(Url::parse("https://api.github.com/x?page=1").unwrap()),
                PageLink::Next(Url::parse("https://api.github.com/x?page=3").unwrap()),
                PageLink::First(Url::parse("https://api.github.com/x?page=1").unwrap()),
                PageLink::Last(Url::parse("https://api.github.com/x?page=9").unwrap()),
            ]
        );
    }

    #[test]
    fn unknown_rel_is_skipped() {
        let header = r#"<https://example.com/spec>; rel="describedby", <https://example.com/p2>; rel="next""#;
        let links = pagination_links(&headers_with_link(header)).unwrap();
        assert_eq!(
            links,
            vec![PageLink::Next(
                Url::parse("https://example.com/p2").unwrap()
            )]
        );
    }

    #[test]
    fn parses_entry_with_extra_params() {
        let mut input = r#"<https://example.com/page2>; rel="next"; type="text/html""#;
        let entry = link_entry(&mut input).unwrap();
        assert_eq!(
            entry,
            LinkEntry {
                url: "https://example.com/page2",
                params: vec![("rel", "next"), ("type", "text/html")],
            }
        );
    }
}
