//! Create Heroku Dev Center changelog entries via the private API.
//!
//! The Dev Center private API exposes `POST /api/v1/private/changelog_items` to
//! create an entry (from a `title`, `content` Markdown body, and optional
//! `published` flag) and `GET /api/v1/private/changelog_items` to list existing
//! entries newest-first (paginated). Both authenticate with HTTP Basic auth
//! where the password is a Heroku OAuth token belonging to an active *admin* Dev
//! Center user (an empty username is sent).
//!
//! Every entry is created through [`create_changelog_item`] (directly or via
//! [`create_and_report`]); the underlying POST is private so a caller cannot
//! bypass the duplicate guard. Creating an entry is not idempotent, so
//! publishing is protected against duplicates: before each POST it scans recent
//! entries for a matching published entry.

use crate::{MAX_RETRY_ATTEMPTS, RETRY_DELAY, with_retries, with_retries_if};
use chrono::{DateTime, TimeDelta, Utc};
use clap::ValueEnum;
use indoc::formatdoc;
use reqwest::{StatusCode, Url};
use std::fmt;
use std::time::Duration;

/// Production Dev Center base URL, parsed once and shared by callers of
/// [`create_changelog_item`] as its `host`.
pub static DEVCENTER_HOST: std::sync::LazyLock<Url> = std::sync::LazyLock::new(|| {
    Url::parse("https://devcenter.heroku.com").expect("hard-coded Dev Center host is a valid URL")
});

/// How far back [`create_changelog_item`] looks for a duplicate when publishing.
/// The scan filters on `created_at`, so an entry created earlier than this is not
/// considered a duplicate even if it was published more recently.
pub const DUPLICATE_WINDOW_DAYS: i64 = 7;

/// The largest page the Dev Center private API will serve (`per_page`), used to
/// scan recent entries in as few requests as possible.
const MAX_PER_PAGE: u32 = 120;

/// A Heroku Dev Center OAuth token
///
/// - Intentionally does not implement: `Display`, `Deref`
/// - Manual [`Debug`](fmt::Debug) implementation to redact internals
/// - Use [`DevCenterToken::as_str`] to obtain the secret
#[derive(Clone, PartialEq, Eq)]
pub struct DevCenterToken(String);

/// Error returned when constructing a [`DevCenterToken`] from invalid input.
#[derive(Debug, PartialEq, thiserror::Error)]
pub enum TokenError {
    /// The provided value was empty (or only whitespace).
    #[error("Dev Center token cannot be empty")]
    CannotBeEmpty,
}

impl DevCenterToken {
    /// Borrow the underlying secret, e.g. to set an `Authorization` header.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<&str> for DevCenterToken {
    type Error = TokenError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        let token = value.trim();
        if token.is_empty() {
            Err(TokenError::CannotBeEmpty)
        } else {
            Ok(DevCenterToken(token.to_owned()))
        }
    }
}

impl fmt::Debug for DevCenterToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("DevCenterToken")
            .field(&"[REDACTED]")
            .finish()
    }
}

/// Publication state for a new changelog entry.
#[derive(Clone, Debug, PartialEq, Eq, ValueEnum)]
pub enum Status {
    Draft,
    Published,
}

/// A changelog entry to create with [`create_changelog_item`].
///
/// The [`status`](Self::status) field switches both the entry's end-user
/// visibility and whether its creation is guarded against duplicates.
///
/// ```no_run
/// use shared::devcenter::{self, DevCenterToken, NewChangelogItem, Status};
///
/// # async fn run() -> Result<(), Box<dyn std::error::Error>> {
/// let token = DevCenterToken::try_from("heroku-oauth-token")?;
/// let host = "https://devcenter.heroku.com".parse()?;
/// let item = NewChangelogItem {
///     title: "Ruby version 3.4.1 is now available".to_string(),
///     content: "Details about the release.".to_string(),
///     status: Status::Draft,
/// };
/// devcenter::create_changelog_item(&host, &token, &item).await?;
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewChangelogItem {
    /// Short headline, rendered by Dev Center as the entry heading.
    pub title: String,
    /// Markdown body (without a repeated heading).
    pub content: String,
    /// Publication state, which switches two coupled behaviors. [`Status::Published`]
    /// is visible to end users and its creation is guarded against duplicates;
    /// [`Status::Draft`] is hidden from end users and is created on every call
    /// without that guard. See [`create_changelog_item`] for the exact rule.
    pub status: Status,
}

/// The subset of the `201 Created` response body that callers consume.
///
/// Unknown fields in the response are ignored.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct CreatedChangelogItem {
    /// The new changelog item's id.
    pub id: u64,
    /// Timestamp the item was published, or `None` when it was created as a draft.
    #[serde(default)]
    pub published_at: Option<DateTime<Utc>>,
}

impl CreatedChangelogItem {
    /// Whether the created item was published (as opposed to left a draft).
    #[must_use]
    pub fn is_published(&self) -> bool {
        self.published_at.is_some()
    }
}

/// An existing changelog entry as returned by the listing endpoint.
///
/// Unknown fields in the response are ignored.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct ExistingChangelogItem {
    /// The changelog item's id.
    pub id: u64,
    /// The entry heading.
    pub title: String,
    /// The Markdown body.
    pub content: String,
    /// When the entry was created (the listing is ordered by this, newest first).
    pub created_at: DateTime<Utc>,
    /// When the entry was published, or `None` while it is an unpublished draft.
    #[serde(default)]
    pub published_at: Option<DateTime<Utc>>,
}

impl ExistingChangelogItem {
    /// Whether this entry has been published (as opposed to being a draft).
    #[must_use]
    pub fn is_published(&self) -> bool {
        self.published_at.is_some()
    }

    /// Whether this entry is the same announcement as `item`: the same title
    /// (ignoring surrounding whitespace) in the same publish state. Content is
    /// deliberately excluded so a regenerated body cannot slip a second
    /// customer-visible entry past the duplicate guard.
    fn matches(&self, item: &NewChangelogItem) -> bool {
        self.title.trim() == item.title.trim()
            && self.is_published() == (item.status == Status::Published)
    }

    /// Represent this already-existing entry as the result of creating it, used
    /// when a retried publish discovers the entry its own earlier attempt created.
    fn into_created(self) -> CreatedChangelogItem {
        CreatedChangelogItem {
            id: self.id,
            published_at: self.published_at,
        }
    }
}

/// One page of the changelog listing endpoint's response.
#[derive(Debug, serde::Deserialize)]
struct ChangelogItemsPage {
    results: Vec<ExistingChangelogItem>,
    /// The next page number, or `None` on the last page.
    #[serde(default)]
    next_page: Option<u32>,
}

/// The result of [`create_changelog_item`].
#[derive(Debug)]
pub enum CreateOutcome {
    /// A new entry was created (a draft, a fresh publish, or a publish recovered
    /// from a retried attempt whose response was lost).
    Created(CreatedChangelogItem),
    /// Publishing was skipped: a matching published entry was created within the
    /// last [`DUPLICATE_WINDOW_DAYS`] days. Nothing was created.
    AlreadyPublished(ExistingChangelogItem),
}

/// Errors that can occur while creating or listing changelog items.
#[derive(Debug, thiserror::Error)]
pub enum DevCenterError {
    /// The request could not be built or sent, or the response could not be read.
    #[error("Failed to reach Dev Center: {0}")]
    Transport(reqwest::Error),

    /// `401 Unauthorized` -- the token is missing, invalid, or not an admin.
    #[error(
        "Dev Center denied access (401). Is HEROKU_DEVCENTER_API_TOKEN a valid, active admin Dev Center token?"
    )]
    AccessDenied,

    /// `422 Unprocessable Entity` -- Dev Center rejected the changelog fields.
    #[error("Dev Center rejected the changelog (422): {body}")]
    Validation {
        /// The raw response body describing the validation failure.
        body: String,
    },

    /// `429 Too Many Requests` -- the private-API rate limit (60/min, 600/hour) was hit.
    #[error("Dev Center rate limit exceeded (429). Try again shortly.")]
    RateLimited,

    /// Any other non-success status.
    #[error("Unexpected Dev Center response {status}: {body}")]
    Unexpected {
        /// The HTTP status returned.
        status: StatusCode,
        /// The raw response body.
        body: String,
    },

    /// A publish POST was accepted but Dev Center left the entry an unpublished
    /// draft, so the publish did not take effect and the entry is not visible.
    #[error(
        "Dev Center accepted the publish but left changelog item {id} an unpublished draft; the publish did not take effect."
    )]
    PublishedAsDraft {
        /// The id of the created-but-unpublished entry.
        id: u64,
    },
}

impl DevCenterError {
    /// Whether retrying the request could plausibly succeed. Transport blips, rate
    /// limiting, and server errors are transient; auth and validation failures are not.
    fn is_retryable(&self) -> bool {
        match self {
            DevCenterError::Transport(_) | DevCenterError::RateLimited => true,
            DevCenterError::Unexpected { status, .. } => status.is_server_error(),
            DevCenterError::AccessDenied
            | DevCenterError::Validation { .. }
            | DevCenterError::PublishedAsDraft { .. } => false,
        }
    }
}

#[derive(serde::Serialize)]
struct RequestBody<'a> {
    changelog_item: Fields<'a>,
}

#[derive(serde::Serialize)]
struct Fields<'a> {
    title: &'a str,
    content: &'a str,
    // Absent => draft. `"true"` => publish now (the Dev Center model treats
    // `"1"`/`"true"` as publish and anything else as a draft).
    #[serde(skip_serializing_if = "Option::is_none")]
    published: Option<&'static str>,
}

/// The shared HTTP client. `reqwest::Client` is internally reference-counted, so
/// cloning this one instance is cheap.
fn client() -> reqwest::Client {
    static CLIENT: std::sync::LazyLock<reqwest::Client> = std::sync::LazyLock::new(|| {
        reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .user_agent("heroku-ruby-builder")
            .build()
            .expect("default reqwest client builds")
    });
    CLIENT.clone()
}

/// Create a changelog entry, refusing to publish a duplicate.
///
/// `host` is the Dev Center base URL (e.g. `https://devcenter.heroku.com`).
/// Exposed for testing.
///
/// A **draft** (`NewChangelogItem.status == Status::Draft`) is always created (duplicate
/// drafts are harmless), which doubles as a check that the API and token work.
/// Only transient failures are retried.
///
/// A **publish** (`NewChangelogItem.status == Status::Published`) first scans entries created within
/// the last [`DUPLICATE_WINDOW_DAYS`] for one matching `item`'s title (ignoring
/// surrounding whitespace) and publish state:
///
/// - If a pre-existing match is found, returns
///   [`AlreadyPublished`](CreateOutcome::AlreadyPublished) **without creating
///   anything**.
/// - Otherwise it POSTs, retrying transient failures. Because the POST is not
///   idempotent, each retry re-scans first: a match created at/after the call
///   began is treated as a previous attempt's success and returned as
///   [`Created`](CreateOutcome::Created) rather than posted again.
///
/// # Errors
///
/// Returns [`DevCenterError`] on transport failure or a non-2xx response (see the
/// variants). A failed duplicate scan fails closed: the entry is not published.
///
/// # Examples
///
/// ```no_run
/// use shared::devcenter::{self, CreateOutcome, DevCenterToken, NewChangelogItem, Status};
///
/// # async fn run() -> Result<(), Box<dyn std::error::Error>> {
/// let token = DevCenterToken::try_from("heroku-oauth-token")?;
/// let host = "https://devcenter.heroku.com".parse()?;
/// let item = NewChangelogItem {
///     title: "Ruby version 3.4.1 is now available".to_string(),
///     content: "Details about the release.".to_string(),
///     status: Status::Draft,
/// };
/// match devcenter::create_changelog_item(&host, &token, &item).await? {
///     CreateOutcome::Created(created) => println!("created id={}", created.id),
///     CreateOutcome::AlreadyPublished(existing) => {
///         println!("already published id={}", existing.id)
///     }
/// }
/// # Ok(())
/// # }
/// ```
pub async fn create_changelog_item(
    host: &Url,
    token: &DevCenterToken,
    item: &NewChangelogItem,
) -> Result<CreateOutcome, DevCenterError> {
    if item.status == Status::Published {
        publish_guarding_duplicates_since(host, token, item, Utc::now()).await
    } else {
        // Allow duplicate draft posts for testing against the live server. They do not
        // show up to customers, but will create real database entries.
        let created = with_retries_if(DevCenterError::is_retryable, || {
            post_changelog_item(host, token, item)
        })
        .await?;
        Ok(CreateOutcome::Created(created))
    }
}

/// Parse `HEROKU_DEVCENTER_API_TOKEN`'s raw value, distinguishing unset,
/// non-UTF-8, and empty/whitespace-only values in the error message.
fn token_from_env(
    value: Option<std::ffi::OsString>,
) -> Result<DevCenterToken, Box<dyn std::error::Error>> {
    let value = value.ok_or_else(|| String::from("HEROKU_DEVCENTER_API_TOKEN is not set"))?;
    let value = value
        .into_string()
        .map_err(|_| String::from("HEROKU_DEVCENTER_API_TOKEN is not valid UTF-8"))?;
    Ok(DevCenterToken::try_from(value.as_str())?)
}

/// Create `item` against [`DEVCENTER_HOST`] using the token from the
/// `HEROKU_DEVCENTER_API_TOKEN` environment variable, reporting the outcome.
///
/// A newly created entry is printed to stdout as success. A skipped duplicate --
/// a matching published entry created within the last [`DUPLICATE_WINDOW_DAYS`]
/// days -- is treated as an error, because an entry that already exists is
/// unexpected (for example, two builds racing to publish the same version).
///
/// # Errors
///
/// Returns an error when `HEROKU_DEVCENTER_API_TOKEN` is unset, when its value
/// is not a usable token, when the Dev Center API call fails, or when publishing
/// is skipped because a matching entry was already published.
pub async fn create_and_report(item: &NewChangelogItem) -> Result<(), Box<dyn std::error::Error>> {
    let token = token_from_env(std::env::var_os("HEROKU_DEVCENTER_API_TOKEN"))?;

    report_outcome(create_changelog_item(&DEVCENTER_HOST, &token, item).await?)
}

/// Turn a [`CreateOutcome`] into a process result.
///
/// A newly created entry is printed and treated as success. An
/// [`AlreadyPublished`](CreateOutcome::AlreadyPublished) match returns an error:
/// an entry that already exists is unexpected (for example, two builds racing to
/// publish the same version) and should fail the run rather than pass silently.
fn report_outcome(outcome: CreateOutcome) -> Result<(), Box<dyn std::error::Error>> {
    match outcome {
        CreateOutcome::Created(created) => {
            println!(
                "Created changelog item id={id} ({state})",
                id = created.id,
                state = if created.is_published() {
                    "published"
                } else {
                    "draft"
                },
            );
            Ok(())
        }
        CreateOutcome::AlreadyPublished(existing) => {
            let id = existing.id;
            let title = existing.title;
            let published_at = existing
                .published_at
                .map_or_else(|| "unknown".to_string(), |at| at.to_rfc3339());
            Err(formatdoc! {"
                A matching published changelog entry was created in the last {DUPLICATE_WINDOW_DAYS} days, so nothing was created. An existing publish is unexpected here (for example, two builds racing to publish the same version) and likely needs manual review.
                  id:           {id}
                  title:        {title}
                  published_at: {published_at}
            "}
            .into())
        }
    }
}

/// Fetch changelog entries created at or after `created_after`, newest first.
///
/// Each page request is retried on failure.
///
/// # Errors
///
/// Returns [`DevCenterError`] on transport failure or a non-2xx response.
pub async fn scan_recent_changelog_items(
    host: &Url,
    token: &DevCenterToken,
    created_after: DateTime<Utc>,
) -> Result<Vec<ExistingChangelogItem>, DevCenterError> {
    let mut items = Vec::new();
    let mut page = 1;

    loop {
        let ChangelogItemsPage { results, next_page } =
            with_retries(|| list_changelog_items_page(host, token, page, MAX_PER_PAGE)).await?;

        let mut reached_window_end = false;
        for item in results {
            if item.created_at < created_after {
                reached_window_end = true;
                break;
            }
            items.push(item);
        }

        match next_page {
            Some(next) if !reached_window_end => page = next,
            _ => return Ok(items),
        }
    }
}

/// Publish `item`, guarding against duplicates relative to `started_at`.
///
/// Split out from [`create_changelog_item`] so `started_at` -- the boundary
/// separating a pre-existing duplicate from this call's own (possibly retried)
/// creation -- can be supplied deterministically in tests.
async fn publish_guarding_duplicates_since(
    host: &Url,
    token: &DevCenterToken,
    item: &NewChangelogItem,
    started_at: DateTime<Utc>,
) -> Result<CreateOutcome, DevCenterError> {
    let window_start = started_at - TimeDelta::days(DUPLICATE_WINDOW_DAYS);

    let mut attempts: u8 = 0;
    let mut posted = false;
    loop {
        attempts += 1;

        // Fails closed: the `?` means a scan error returns without POSTing, so a
        // failed scan can never let a duplicate publish through.
        let recent = scan_recent_changelog_items(host, token, window_start).await?;

        let mut preexisting = None;
        for candidate in recent {
            if candidate.matches(item) {
                if posted && candidate.created_at >= started_at {
                    // A POST has gone out and the match postdates this call's
                    // start, so it is that POST's own entry resurfacing after its
                    // response was lost. Adopt it rather than POST a duplicate.
                    // Before any POST, an entry postdating the start is instead a
                    // concurrent racer or clock skew and stays a duplicate below.
                    return Ok(CreateOutcome::Created(candidate.into_created()));
                }
                preexisting.get_or_insert(candidate);
            }
        }
        if let Some(existing) = preexisting {
            return Ok(CreateOutcome::AlreadyPublished(existing));
        }

        match post_changelog_item(host, token, item).await {
            // A publish that comes back unpublished is an invisible draft, not a
            // success; retrying would POST a duplicate, so fail hard instead.
            Ok(created) if !created.is_published() => {
                return Err(DevCenterError::PublishedAsDraft { id: created.id });
            }
            Ok(created) => return Ok(CreateOutcome::Created(created)),
            Err(error) if attempts < MAX_RETRY_ATTEMPTS && error.is_retryable() => {
                posted = true;
                tokio::time::sleep(RETRY_DELAY).await;
            }
            Err(error) => return Err(error),
        }
    }
}

/// Read a response body for use in an error message, substituting a sentinel
/// when the body itself cannot be read (so the failure is never silent/blank).
async fn error_body(response: reqwest::Response) -> String {
    response
        .text()
        .await
        .unwrap_or_else(|err| format!("<failed to read response body: {err}>"))
}

/// Fetch a single page of the changelog listing endpoint.
///
/// Entries are ordered newest first (`created_at` descending), across pages as
/// well as within one: every entry on a later page is older than every entry on
/// an earlier one.
async fn list_changelog_items_page(
    host: &Url,
    token: &DevCenterToken,
    page: u32,
    per_page: u32,
) -> Result<ChangelogItemsPage, DevCenterError> {
    let mut url = host.clone();
    url.set_path("/api/v1/private/changelog_items");
    url.query_pairs_mut()
        .append_pair("page", &page.to_string())
        .append_pair("per_page", &per_page.to_string());

    let response = client()
        .get(url)
        .basic_auth("", Some(token.as_str()))
        .header(reqwest::header::ACCEPT, "application/json")
        .send()
        .await
        .map_err(DevCenterError::Transport)?;

    let status = response.status();
    match status {
        StatusCode::OK => response
            .json::<ChangelogItemsPage>()
            .await
            .map_err(DevCenterError::Transport),
        StatusCode::UNAUTHORIZED => Err(DevCenterError::AccessDenied),
        StatusCode::TOO_MANY_REQUESTS => Err(DevCenterError::RateLimited),
        _ => Err(DevCenterError::Unexpected {
            status,
            body: error_body(response).await,
        }),
    }
}

/// POST a changelog item to `{host}/api/v1/private/changelog_items`.
///
/// Private on purpose: publishing must go through [`create_changelog_item`] so it
/// cannot skip the duplicate guard.
async fn post_changelog_item(
    host: &Url,
    token: &DevCenterToken,
    item: &NewChangelogItem,
) -> Result<CreatedChangelogItem, DevCenterError> {
    let mut url = host.clone();
    url.set_path("/api/v1/private/changelog_items");
    let body = RequestBody {
        changelog_item: Fields {
            title: &item.title,
            content: &item.content,
            published: (item.status == Status::Published).then_some("true"),
        },
    };

    let response = client()
        .post(url)
        .basic_auth("", Some(token.as_str()))
        .header(reqwest::header::ACCEPT, "application/json")
        .json(&body)
        .send()
        .await
        .map_err(DevCenterError::Transport)?;

    let status = response.status();
    match status {
        StatusCode::CREATED | StatusCode::OK => response
            .json::<CreatedChangelogItem>()
            .await
            .map_err(DevCenterError::Transport),
        StatusCode::UNAUTHORIZED => Err(DevCenterError::AccessDenied),
        StatusCode::UNPROCESSABLE_ENTITY => Err(DevCenterError::Validation {
            body: error_body(response).await,
        }),
        StatusCode::TOO_MANY_REQUESTS => Err(DevCenterError::RateLimited),
        _ => Err(DevCenterError::Unexpected {
            status,
            body: error_body(response).await,
        }),
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::thread;
    use tiny_http::{Response, Server};

    struct CapturedRequest {
        method: String,
        url: String,
        authorization: Option<String>,
        accept: Option<String>,
        body: String,
    }

    fn header(request: &tiny_http::Request, name: &'static str) -> Option<String> {
        request
            .headers()
            .iter()
            .find(|h| h.field.equiv(name))
            .map(|h| h.value.as_str().to_string())
    }

    /// Start a local server that serves every request by asking `handler` for a
    /// `(status, body)` from the request's method and url. Returns its base URL
    /// and the requests it has captured (each recorded before its response is
    /// sent, so they are all present once the client call returns).
    ///
    /// Uses a small worker pool so a keep-alive connection idling between
    /// requests (as the shared client's pooled connection does) cannot block
    /// serving another connection.
    fn spawn_router<H>(handler: H) -> (Url, Arc<Mutex<Vec<CapturedRequest>>>)
    where
        H: Fn(&str, &str) -> (u16, String) + Send + Sync + 'static,
    {
        let server = Arc::new(Server::http("127.0.0.1:0").unwrap());
        let addr = Url::parse(&format!("http://{}", server.server_addr())).unwrap();
        let captured = Arc::new(Mutex::new(Vec::new()));
        let handler = Arc::new(handler);

        for _ in 0..4 {
            let server = Arc::clone(&server);
            let captured = Arc::clone(&captured);
            let handler = Arc::clone(&handler);
            thread::spawn(move || {
                for mut request in server.incoming_requests() {
                    let method = request.method().as_str().to_string();
                    let url = request.url().to_string();
                    let authorization = header(&request, "Authorization");
                    let accept = header(&request, "Accept");
                    let mut body = String::new();
                    std::io::Read::read_to_string(request.as_reader(), &mut body).unwrap();

                    let (status, response_body) = handler(&method, &url);
                    captured.lock().unwrap().push(CapturedRequest {
                        method,
                        url,
                        authorization,
                        accept,
                        body,
                    });
                    let response = Response::from_string(response_body)
                        .with_status_code(tiny_http::StatusCode(status));
                    let _ = request.respond(response);
                }
            });
        }

        (addr, captured)
    }

    fn token() -> DevCenterToken {
        DevCenterToken::try_from("secret-token").unwrap()
    }

    fn draft(title: &str, content: &str) -> NewChangelogItem {
        NewChangelogItem {
            title: title.to_string(),
            content: content.to_string(),
            status: Status::Draft,
        }
    }

    fn publish(title: &str, content: &str) -> NewChangelogItem {
        NewChangelogItem {
            title: title.to_string(),
            content: content.to_string(),
            status: Status::Published,
        }
    }

    fn at(rfc3339: &str) -> DateTime<Utc> {
        rfc3339.parse().unwrap()
    }

    #[test]
    fn token_rejects_empty_strings() {
        assert!(matches!(
            DevCenterToken::try_from(""),
            Err(TokenError::CannotBeEmpty)
        ));
        assert!(matches!(
            DevCenterToken::try_from("   "),
            Err(TokenError::CannotBeEmpty)
        ));
    }

    #[test]
    fn token_from_env_errors_when_unset() {
        let error = token_from_env(None).unwrap_err();
        assert!(error.to_string().contains("is not set"));
    }

    #[test]
    fn token_from_env_rejects_empty_or_whitespace() {
        for value in ["", "   "] {
            let error = token_from_env(Some(std::ffi::OsString::from(value))).unwrap_err();
            assert!(
                error.to_string().contains("cannot be empty"),
                "value {value:?} produced: {error}"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn token_from_env_errors_on_non_utf8() {
        use std::os::unix::ffi::OsStringExt;

        let value = std::ffi::OsString::from_vec(vec![0xff, 0xff]);
        let error = token_from_env(Some(value)).unwrap_err();
        assert!(error.to_string().contains("not valid UTF-8"));
    }

    #[test]
    fn token_from_env_accepts_a_valid_value() {
        let token = token_from_env(Some(std::ffi::OsString::from("secret-token"))).unwrap();
        assert_eq!(token.as_str(), "secret-token");
    }

    #[cfg(unix)]
    #[test]
    fn token_from_env_failure_modes_are_distinguishable() {
        use std::os::unix::ffi::OsStringExt;

        let unset = token_from_env(None).unwrap_err().to_string();
        let empty = token_from_env(Some(std::ffi::OsString::from("   ")))
            .unwrap_err()
            .to_string();
        let non_utf8 = token_from_env(Some(std::ffi::OsString::from_vec(vec![0xff, 0xff])))
            .unwrap_err()
            .to_string();

        assert_ne!(unset, empty);
        assert_ne!(unset, non_utf8);
        assert_ne!(empty, non_utf8);
    }

    #[test]
    fn debug_redacts_token_secret() {
        let token = DevCenterToken::try_from("supersecret").unwrap();
        let debug = format!("{token:?}");
        assert!(
            !debug.contains("supersecret"),
            "DevCenterToken Debug output leaked the secret: {debug}"
        );
    }

    #[test]
    fn devcenter_host_is_the_production_url() {
        assert_eq!(DEVCENTER_HOST.as_str(), "https://devcenter.heroku.com/");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn posts_a_draft_and_parses_the_response() {
        let (addr, requests) =
            spawn_router(|_method, _url| (201, r#"{"id":42,"published_at":null}"#.to_string()));

        let created = post_changelog_item(&addr, &token(), &draft("A title", "Some content"))
            .await
            .unwrap();

        assert_eq!(created.id, 42);
        assert!(!created.is_published());

        let requests = requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        let request = &requests[0];
        assert_eq!(request.method, "POST");
        assert_eq!(request.url, "/api/v1/private/changelog_items");
        assert_eq!(request.accept.as_deref(), Some("application/json"));
        assert!(
            request
                .authorization
                .as_deref()
                .is_some_and(|value| value.starts_with("Basic ")),
            "expected HTTP Basic auth, got {:?}",
            request.authorization
        );
        assert!(request.body.contains("\"title\":\"A title\""));
        assert!(request.body.contains("\"content\":\"Some content\""));
        // A draft omits the `published` field entirely.
        assert!(!request.body.contains("published"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn publishing_sends_the_published_field() {
        let (addr, requests) = spawn_router(|_method, _url| {
            (
                201,
                r#"{"id":7,"published_at":"2026-09-15T00:00:00Z"}"#.to_string(),
            )
        });

        let created = post_changelog_item(&addr, &token(), &publish("T", "C"))
            .await
            .unwrap();

        assert!(created.is_published());
        let requests = requests.lock().unwrap();
        assert!(requests[0].body.contains("\"published\":\"true\""));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn created_response_rejects_a_blank_published_at() {
        // The API sends `null` or a real timestamp; a blank string is malformed
        // and must not be read as "published".
        let (addr, _requests) =
            spawn_router(|_method, _url| (201, r#"{"id":1,"published_at":""}"#.to_string()));

        let error = post_changelog_item(&addr, &token(), &publish("T", "C"))
            .await
            .unwrap_err();

        assert!(matches!(error, DevCenterError::Transport(_)));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn maps_401_to_access_denied() {
        let (addr, _requests) =
            spawn_router(|_method, _url| (401, r#"{"error":"Access denied"}"#.to_string()));

        let error = post_changelog_item(&addr, &token(), &draft("t", "c"))
            .await
            .unwrap_err();

        assert!(matches!(error, DevCenterError::AccessDenied));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn maps_422_and_preserves_body() {
        let (addr, _requests) =
            spawn_router(|_method, _url| (422, r#"{"content":["can't be blank"]}"#.to_string()));

        let error = post_changelog_item(&addr, &token(), &draft("t", "c"))
            .await
            .unwrap_err();

        match error {
            DevCenterError::Validation { body } => assert!(body.contains("can't be blank")),
            other => panic!("expected Validation, got {other:?}"),
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn maps_429_to_rate_limited() {
        let (addr, _requests) =
            spawn_router(|_method, _url| (429, r#"{"error":"Rate limit exceeded"}"#.to_string()));

        let error = post_changelog_item(&addr, &token(), &draft("t", "c"))
            .await
            .unwrap_err();

        assert!(matches!(error, DevCenterError::RateLimited));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn scans_all_pages_following_next_page() {
        let (addr, requests) = spawn_router(|method, url| {
            assert_eq!(method, "GET");
            if url.contains("?page=1") {
                (
                    200,
                    r#"{"results":[{"id":1,"title":"A","content":"a","created_at":"2026-09-20T00:00:00Z","published_at":"2026-09-20T00:00:00Z"}],"next_page":2}"#
                        .to_string(),
                )
            } else if url.contains("?page=2") {
                (
                    200,
                    r#"{"results":[{"id":2,"title":"B","content":"b","created_at":"2026-09-19T00:00:00Z","published_at":null}],"next_page":null}"#
                        .to_string(),
                )
            } else {
                (500, "unexpected page".to_string())
            }
        });

        let items = scan_recent_changelog_items(&addr, &token(), at("2000-01-01T00:00:00Z"))
            .await
            .unwrap();

        assert_eq!(
            items.iter().map(|item| item.id).collect::<Vec<_>>(),
            vec![1, 2]
        );
        let requests = requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        assert!(requests[0].url.contains("?page=1"));
        assert!(requests[0].url.contains("per_page=120"));
        assert!(requests[1].url.contains("?page=2"));
        assert_eq!(requests[0].accept.as_deref(), Some("application/json"));
        assert!(
            requests[0]
                .authorization
                .as_deref()
                .is_some_and(|value| value.starts_with("Basic ")),
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn scan_stops_at_window_boundary_without_fetching_next_page() {
        let (addr, requests) = spawn_router(|_method, url| {
            if url.contains("?page=1") {
                (
                    200,
                    r#"{"results":[
                        {"id":1,"title":"recent","content":"r","created_at":"2026-09-20T00:00:00Z","published_at":"2026-09-20T00:00:00Z"},
                        {"id":2,"title":"old","content":"o","created_at":"2026-09-01T00:00:00Z","published_at":"2026-09-01T00:00:00Z"}
                    ],"next_page":2}"#
                        .to_string(),
                )
            } else {
                (500, "should not fetch page 2".to_string())
            }
        });

        let items = scan_recent_changelog_items(&addr, &token(), at("2026-09-13T12:00:00Z"))
            .await
            .unwrap();

        assert_eq!(
            items.iter().map(|item| item.id).collect::<Vec<_>>(),
            vec![1]
        );
        let requests = requests.lock().unwrap();
        assert_eq!(requests.len(), 1, "must stop before fetching page 2");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn scan_maps_unauthorized_to_access_denied() {
        let (addr, _requests) =
            spawn_router(|_method, _url| (401, r#"{"error":"Access denied"}"#.to_string()));

        let error = scan_recent_changelog_items(&addr, &token(), at("2000-01-01T00:00:00Z"))
            .await
            .unwrap_err();

        assert!(matches!(error, DevCenterError::AccessDenied));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn draft_creation_creates_without_scanning() {
        let (addr, requests) =
            spawn_router(|_method, _url| (201, r#"{"id":99,"published_at":null}"#.to_string()));

        let outcome = create_changelog_item(&addr, &token(), &draft("t", "c"))
            .await
            .unwrap();

        match outcome {
            CreateOutcome::Created(created) => assert_eq!(created.id, 99),
            other => panic!("expected Created, got {other:?}"),
        }
        let requests = requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].method, "POST");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn draft_does_not_retry_a_non_retryable_error() {
        let post_calls = Arc::new(AtomicUsize::new(0));
        let post_counter = Arc::clone(&post_calls);

        let (addr, _requests) = spawn_router(move |_method, _url| {
            post_counter.fetch_add(1, Ordering::SeqCst);
            (401, r#"{"error":"Access denied"}"#.to_string())
        });

        let error = create_changelog_item(&addr, &token(), &draft("t", "c"))
            .await
            .unwrap_err();

        assert!(matches!(error, DevCenterError::AccessDenied));
        assert_eq!(
            post_calls.load(Ordering::SeqCst),
            1,
            "a non-retryable error must not be retried"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn publishing_without_a_duplicate_creates_the_entry() {
        let (addr, requests) = spawn_router(|method, _url| match method {
            "GET" => (200, r#"{"results":[],"next_page":null}"#.to_string()),
            _ => (
                201,
                r#"{"id":100,"published_at":"2026-09-21T00:00:00Z"}"#.to_string(),
            ),
        });

        let outcome = create_changelog_item(&addr, &token(), &publish("New", "body"))
            .await
            .unwrap();

        match outcome {
            CreateOutcome::Created(created) => {
                assert_eq!(created.id, 100);
                assert!(created.is_published());
            }
            other => panic!("expected Created, got {other:?}"),
        }
        let requests = requests.lock().unwrap();
        assert_eq!(requests[0].method, "GET");
        assert!(requests.iter().any(|request| request.method == "POST"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn publish_errors_when_the_api_returns_an_unpublished_entry() {
        // The POST is accepted (201) but the entry comes back unpublished, so the
        // publish silently did not take effect.
        let (addr, _requests) = spawn_router(|method, _url| match method {
            "GET" => (200, r#"{"results":[],"next_page":null}"#.to_string()),
            _ => (201, r#"{"id":55,"published_at":null}"#.to_string()),
        });

        let error = publish_guarding_duplicates_since(
            &addr,
            &token(),
            &publish("Ruby 3.4.1", "body"),
            at("2026-09-20T12:00:00Z"),
        )
        .await
        .unwrap_err();

        assert!(
            matches!(error, DevCenterError::PublishedAsDraft { id: 55 }),
            "an accepted-but-unpublished publish must be a hard error, got {error:?}"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn publish_refuses_a_preexisting_published_duplicate() {
        let (addr, requests) = spawn_router(|method, _url| {
            if method == "GET" {
                (
                    200,
                    r#"{"results":[{"id":5,"title":"Ruby 3.4.1","content":"body","created_at":"2026-09-20T00:00:00Z","published_at":"2026-09-20T00:00:00Z"}],"next_page":null}"#
                        .to_string(),
                )
            } else {
                (500, "should not POST".to_string())
            }
        });

        let outcome = publish_guarding_duplicates_since(
            &addr,
            &token(),
            &publish("Ruby 3.4.1", "body"),
            at("2026-09-20T12:00:00Z"),
        )
        .await
        .unwrap();

        match outcome {
            CreateOutcome::AlreadyPublished(existing) => assert_eq!(existing.id, 5),
            other => panic!("expected AlreadyPublished, got {other:?}"),
        }
        let requests = requests.lock().unwrap();
        assert!(
            requests.iter().all(|request| request.method == "GET"),
            "must not POST when a duplicate already exists"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn publish_recovers_its_own_entry_from_a_lost_attempt() {
        let post_calls = Arc::new(AtomicUsize::new(0));
        let get_calls = Arc::new(AtomicUsize::new(0));
        let post_counter = Arc::clone(&post_calls);
        let get_counter = Arc::clone(&get_calls);

        let (addr, _requests) = spawn_router(move |method, _url| {
            if method == "GET" {
                if get_counter.fetch_add(1, Ordering::SeqCst) == 0 {
                    (200, r#"{"results":[],"next_page":null}"#.to_string())
                } else {
                    (
                        200,
                        r#"{"results":[{"id":77,"title":"Ruby 3.4.1","content":"body","created_at":"2026-09-20T13:00:00Z","published_at":"2026-09-20T13:00:00Z"}],"next_page":null}"#
                            .to_string(),
                    )
                }
            } else {
                post_counter.fetch_add(1, Ordering::SeqCst);
                (429, r#"{"error":"Rate limit exceeded"}"#.to_string())
            }
        });

        let outcome = publish_guarding_duplicates_since(
            &addr,
            &token(),
            &publish("Ruby 3.4.1", "body"),
            at("2026-09-20T12:00:00Z"),
        )
        .await
        .unwrap();

        match outcome {
            CreateOutcome::Created(created) => assert_eq!(created.id, 77),
            other => panic!("expected Created (recovered), got {other:?}"),
        }
        assert_eq!(
            post_calls.load(Ordering::SeqCst),
            1,
            "must not POST again after recovering our own entry"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn publish_does_not_adopt_a_match_on_the_first_scan() {
        let post_calls = Arc::new(AtomicUsize::new(0));
        let post_counter = Arc::clone(&post_calls);

        // The match's `created_at` postdates `started_at` (a concurrent racer, or
        // clock skew between the runner and the Dev Center server), yet no POST has
        // been issued, so it cannot be this call's own lost attempt.
        let (addr, _requests) = spawn_router(move |method, _url| {
            if method == "GET" {
                (
                    200,
                    r#"{"results":[{"id":13,"title":"Ruby 3.4.1","content":"body","created_at":"2026-09-20T13:00:00Z","published_at":"2026-09-20T13:00:00Z"}],"next_page":null}"#
                        .to_string(),
                )
            } else {
                post_counter.fetch_add(1, Ordering::SeqCst);
                (500, "must not POST past a first-scan duplicate".to_string())
            }
        });

        let outcome = publish_guarding_duplicates_since(
            &addr,
            &token(),
            &publish("Ruby 3.4.1", "body"),
            at("2026-09-20T12:00:00Z"),
        )
        .await
        .unwrap();

        match outcome {
            CreateOutcome::AlreadyPublished(existing) => assert_eq!(existing.id, 13),
            other => panic!("expected AlreadyPublished, got {other:?}"),
        }
        assert_eq!(
            post_calls.load(Ordering::SeqCst),
            0,
            "a match on the first scan is a pre-existing publish, not our own lost attempt"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn publish_retries_a_transient_post_error_then_succeeds() {
        let post_calls = Arc::new(AtomicUsize::new(0));
        let post_counter = Arc::clone(&post_calls);

        let (addr, _requests) = spawn_router(move |method, _url| {
            if method == "GET" {
                (200, r#"{"results":[],"next_page":null}"#.to_string())
            } else if post_counter.fetch_add(1, Ordering::SeqCst) == 0 {
                (429, r#"{"error":"Rate limit exceeded"}"#.to_string())
            } else {
                (
                    201,
                    r#"{"id":88,"published_at":"2026-09-20T12:30:00Z"}"#.to_string(),
                )
            }
        });

        let outcome = publish_guarding_duplicates_since(
            &addr,
            &token(),
            &publish("Ruby 3.4.1", "body"),
            at("2026-09-20T12:00:00Z"),
        )
        .await
        .unwrap();

        match outcome {
            CreateOutcome::Created(created) => assert_eq!(created.id, 88),
            other => panic!("expected Created, got {other:?}"),
        }
        assert_eq!(post_calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn publish_gives_up_after_max_retries() {
        let (addr, _requests) = spawn_router(|method, _url| match method {
            "GET" => (200, r#"{"results":[],"next_page":null}"#.to_string()),
            _ => (429, r#"{"error":"Rate limit exceeded"}"#.to_string()),
        });

        let error = publish_guarding_duplicates_since(
            &addr,
            &token(),
            &publish("t", "c"),
            at("2026-09-20T12:00:00Z"),
        )
        .await
        .unwrap_err();

        assert!(matches!(error, DevCenterError::RateLimited));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn publish_fails_closed_when_the_scan_fails() {
        let (addr, requests) = spawn_router(|method, _url| match method {
            "GET" => (401, r#"{"error":"Access denied"}"#.to_string()),
            _ => (
                201,
                r#"{"id":1,"published_at":"2026-09-20T00:00:00Z"}"#.to_string(),
            ),
        });

        let error = publish_guarding_duplicates_since(
            &addr,
            &token(),
            &publish("t", "c"),
            at("2026-09-20T12:00:00Z"),
        )
        .await
        .unwrap_err();

        assert!(matches!(error, DevCenterError::AccessDenied));
        let requests = requests.lock().unwrap();
        assert!(
            requests.iter().all(|request| request.method == "GET"),
            "must not POST when the duplicate scan fails"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn publish_ignores_nonmatching_titles_and_unpublished_drafts() {
        let (addr, requests) = spawn_router(|method, _url| {
            if method == "GET" {
                (
                    200,
                    r#"{"results":[
                    {"id":1,"title":"Different","content":"body","created_at":"2026-09-20T00:00:00Z","published_at":"2026-09-20T00:00:00Z"},
                    {"id":2,"title":"Ruby 3.4.1","content":"body","created_at":"2026-09-20T01:00:00Z","published_at":null}
                ],"next_page":null}"#
                        .to_string(),
                )
            } else {
                (
                    201,
                    r#"{"id":9,"published_at":"2026-09-20T12:30:00Z"}"#.to_string(),
                )
            }
        });

        let outcome = publish_guarding_duplicates_since(
            &addr,
            &token(),
            &publish("Ruby 3.4.1", "body"),
            at("2026-09-20T12:00:00Z"),
        )
        .await
        .unwrap();

        match outcome {
            CreateOutcome::Created(created) => assert_eq!(created.id, 9),
            other => panic!("expected Created, got {other:?}"),
        }
        let requests = requests.lock().unwrap();
        assert!(requests.iter().any(|request| request.method == "POST"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn publish_refuses_a_duplicate_title_with_different_content() {
        let (addr, requests) = spawn_router(|method, _url| {
            if method == "GET" {
                (
                    200,
                    r#"{"results":[{"id":5,"title":"Ruby 3.4.1","content":"stored body","created_at":"2026-09-20T00:00:00Z","published_at":"2026-09-20T00:00:00Z"}],"next_page":null}"#
                        .to_string(),
                )
            } else {
                (
                    201,
                    r#"{"id":100,"published_at":"2026-09-20T12:30:00Z"}"#.to_string(),
                )
            }
        });

        let outcome = publish_guarding_duplicates_since(
            &addr,
            &token(),
            &publish("Ruby 3.4.1", "regenerated body"),
            at("2026-09-20T12:00:00Z"),
        )
        .await
        .unwrap();

        match outcome {
            CreateOutcome::AlreadyPublished(existing) => assert_eq!(existing.id, 5),
            other => panic!("expected AlreadyPublished, got {other:?}"),
        }
        let requests = requests.lock().unwrap();
        assert!(
            requests.iter().all(|request| request.method == "GET"),
            "a published entry with the same title is a duplicate even if the body differs"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn publish_refuses_a_duplicate_title_differing_only_by_whitespace() {
        let (addr, requests) = spawn_router(|method, _url| {
            if method == "GET" {
                (
                    200,
                    r#"{"results":[{"id":6,"title":"  Ruby 3.4.1\n","content":"body","created_at":"2026-09-20T00:00:00Z","published_at":"2026-09-20T00:00:00Z"}],"next_page":null}"#
                        .to_string(),
                )
            } else {
                (
                    201,
                    r#"{"id":101,"published_at":"2026-09-20T12:30:00Z"}"#.to_string(),
                )
            }
        });

        let outcome = publish_guarding_duplicates_since(
            &addr,
            &token(),
            &publish("Ruby 3.4.1", "body"),
            at("2026-09-20T12:00:00Z"),
        )
        .await
        .unwrap();

        match outcome {
            CreateOutcome::AlreadyPublished(existing) => assert_eq!(existing.id, 6),
            other => panic!("expected AlreadyPublished, got {other:?}"),
        }
        let requests = requests.lock().unwrap();
        assert!(
            requests.iter().all(|request| request.method == "GET"),
            "titles differing only by surrounding whitespace are the same announcement"
        );
    }

    #[test]
    fn already_published_is_reported_as_an_error() {
        let outcome = CreateOutcome::AlreadyPublished(ExistingChangelogItem {
            id: 5,
            title: "Ruby 3.4.1".to_string(),
            content: "body".to_string(),
            created_at: at("2026-09-20T00:00:00Z"),
            published_at: Some(at("2026-09-20T00:00:00Z")),
        });

        assert!(
            report_outcome(outcome).is_err(),
            "an already-published match is unexpected (e.g. a race) and must surface a non-zero exit"
        );
    }

    #[test]
    fn a_created_entry_is_reported_as_success() {
        let outcome = CreateOutcome::Created(CreatedChangelogItem {
            id: 100,
            published_at: Some(at("2026-09-21T00:00:00Z")),
        });

        assert!(report_outcome(outcome).is_ok());
    }
}
