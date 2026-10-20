//! Read and publish Dev Center articles.
//!
//! API source: <https://github.com/heroku/devcenter/blob/main/app/controllers/api/private/articles_controller.rb>

use super::{DevCenterError, DevCenterToken, client, error_body};
use crate::with_retries_if;
use reqwest::{StatusCode, Url};
use std::collections::BTreeMap;
use std::fmt;

/// Identifies a Dev Center article.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArticleId(pub u64);

impl fmt::Display for ArticleId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// An article as read from Dev Center.
///
/// Unknown fields in the response are ignored.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub struct Article {
    /// Markdown source.
    pub content: String,
    /// Count of saved revisions. A change between two reads means someone else edited the article.
    pub versions_count: u64,
}

/// The outcome of [`publish_article`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Publication {
    /// The edit returned no new content, so nothing was written.
    Unchanged { versions_count: u64 },
    Published {
        /// Public URL of the article.
        url: Url,
        /// The article's revision count after the save.
        versions_count: u64,
    },
}

/// Validation errors Dev Center reported for an article, keyed by attribute.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub struct ModelErrors(BTreeMap<String, Vec<String>>);

impl fmt::Display for ModelErrors {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let messages = self
            .0
            .iter()
            .flat_map(|(attribute, messages)| {
                messages
                    .iter()
                    .map(move |message| format!("{attribute} {message}"))
            })
            .collect::<Vec<_>>();
        write!(f, "{}", messages.join(", "))
    }
}

#[derive(serde::Serialize)]
struct RequestBody<'a> {
    article: Fields<'a>,
}

#[derive(serde::Serialize)]
struct Fields<'a> {
    content: &'a str,
}

#[derive(serde::Deserialize)]
struct UpdatedArticle {
    url: Url,
}

fn article_url(host: &Url, id: ArticleId) -> Url {
    let mut url = host.clone();
    url.set_path(&format!("/api/v1/private/articles/{id}"));
    url
}

fn validate_url(host: &Url, id: ArticleId) -> Url {
    let mut url = article_url(host, id);
    url.path_segments_mut()
        .expect("HTTP URLs have a path")
        .push("validate");
    url
}

async fn status_error(response: reqwest::Response) -> DevCenterError {
    match response.status() {
        StatusCode::UNAUTHORIZED => DevCenterError::AccessDenied,
        StatusCode::TOO_MANY_REQUESTS => DevCenterError::RateLimited,
        status => DevCenterError::Unexpected {
            status,
            body: error_body(response).await,
        },
    }
}

/// Model errors when `body` holds any, otherwise an unexpected response.
fn rejection(id: ArticleId, status: StatusCode, body: String) -> DevCenterError {
    match serde_json::from_str::<ModelErrors>(&body) {
        Ok(errors) if !errors.0.is_empty() => DevCenterError::ArticleRejected { id, errors },
        _ => DevCenterError::Unexpected { status, body },
    }
}

/// Fetch the article with `id`, retrying transient failures.
///
/// # Errors
///
/// Returns [`DevCenterError`] on transport failure or a non-`200` response.
pub async fn get_article(
    host: &Url,
    token: &DevCenterToken,
    id: ArticleId,
) -> Result<Article, DevCenterError> {
    with_retries_if(DevCenterError::is_retryable, || async {
        let response = client()
            .get(article_url(host, id))
            .basic_auth("", Some(token.as_str()))
            .header(reqwest::header::ACCEPT, "application/json")
            .send()
            .await
            .map_err(DevCenterError::Transport)?;

        if response.status() == StatusCode::OK {
            response
                .json::<Article>()
                .await
                .map_err(DevCenterError::Transport)
        } else {
            Err(status_error(response).await)
        }
    })
    .await
}

/// Ask Dev Center whether `content` would save, without saving it.
async fn validate_article(
    host: &Url,
    token: &DevCenterToken,
    id: ArticleId,
    content: &str,
) -> Result<(), DevCenterError> {
    with_retries_if(DevCenterError::is_retryable, || async {
        let response = client()
            .post(validate_url(host, id))
            .basic_auth("", Some(token.as_str()))
            .header(reqwest::header::ACCEPT, "application/json")
            .json(&RequestBody {
                article: Fields { content },
            })
            .send()
            .await
            .map_err(DevCenterError::Transport)?;

        let status = response.status();
        if status != StatusCode::OK {
            return Err(status_error(response).await);
        }
        let body = error_body(response).await;
        match serde_json::from_str::<ModelErrors>(&body) {
            Ok(errors) if errors.0.is_empty() => Ok(()),
            _ => Err(rejection(id, status, body)),
        }
    })
    .await
}

/// Save `content` to the live article. Private so a write cannot skip the guards in [`publish_article`].
async fn update_article(
    host: &Url,
    token: &DevCenterToken,
    id: ArticleId,
    content: &str,
) -> Result<UpdatedArticle, DevCenterError> {
    with_retries_if(DevCenterError::is_retryable, || async {
        let response = client()
            .put(article_url(host, id))
            .basic_auth("", Some(token.as_str()))
            .header(reqwest::header::ACCEPT, "application/json")
            .json(&RequestBody {
                article: Fields { content },
            })
            .send()
            .await
            .map_err(DevCenterError::Transport)?;

        match response.status() {
            status @ (StatusCode::OK | StatusCode::UNPROCESSABLE_ENTITY) => {
                let body = error_body(response).await;
                serde_json::from_str::<UpdatedArticle>(&body)
                    .map_err(|_| rejection(id, status, body))
            }
            _ => Err(status_error(response).await),
        }
    })
    .await
}

/// Replace the content of the article with `id` with the result of `edit`.
///
/// `edit` receives the article's current content and returns the replacement, or `None` to leave it unchanged. The
/// saved content is always derived from the article being written, so `edit` can refuse an article it does not
/// recognize by returning an error.
///
/// The new content is validated before saving. The save is skipped with [`DevCenterError::ArticleChanged`] when the
/// article gained a revision after it was read, and the article is read back afterwards to confirm the save
/// ([`DevCenterError::ArticleNotStored`]). The API has no optimistic locking, so an edit landing between the re-read
/// and the save can still be lost.
///
/// # Errors
///
/// Returns the error from `edit`, or a [`DevCenterError`] when a guard fails, Dev Center rejects the content, or a
/// request fails.
pub async fn publish_article<E>(
    host: &Url,
    token: &DevCenterToken,
    id: ArticleId,
    edit: impl FnOnce(&str) -> Result<Option<String>, E>,
) -> Result<Publication, E>
where
    E: From<DevCenterError>,
{
    let base = get_article(host, token, id).await?;
    let Some(content) = edit(&base.content)? else {
        return Ok(Publication::Unchanged {
            versions_count: base.versions_count,
        });
    };

    validate_article(host, token, id, &content).await?;

    let current = get_article(host, token, id).await?;
    if current.versions_count != base.versions_count {
        return Err(DevCenterError::ArticleChanged {
            id,
            expected: base.versions_count,
            found: current.versions_count,
        }
        .into());
    }

    let updated = update_article(host, token, id, &content).await?;

    let stored = get_article(host, token, id).await?;
    if stored.content != content {
        return Err(DevCenterError::ArticleNotStored { id }.into());
    }

    Ok(Publication::Published {
        url: updated.url,
        versions_count: stored.versions_count,
    })
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::devcenter::test::{spawn_router, token};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn article_json(content: &str, versions_count: u64) -> String {
        serde_json::json!({
            "id": 698,
            "title": "Heroku Ruby Support",
            "content": content,
            "versions_count": versions_count,
        })
        .to_string()
    }

    fn replace_with(content: &str) -> impl FnOnce(&str) -> Result<Option<String>, DevCenterError> {
        let content = content.to_string();
        move |_| Ok(Some(content))
    }

    const UPDATED: &str = r#"{"title":"Heroku Ruby Support","status":"published","url":"https://devcenter.heroku.com/articles/ruby-support-reference","owner_email":"owner@example.com"}"#;

    fn writes(requests: &[crate::devcenter::test::CapturedRequest]) -> Vec<&str> {
        requests
            .iter()
            .map(|request| request.method.as_str())
            .filter(|method| *method != "GET")
            .collect()
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn get_article_reads_content_and_versions_count() {
        let (addr, requests) = spawn_router(|_, _| (200, article_json("a\r\nb", 487)));

        let article = get_article(&addr, &token(), ArticleId(698)).await.unwrap();

        assert_eq!(
            article,
            Article {
                content: "a\r\nb".to_string(),
                versions_count: 487
            }
        );
        let requests = requests.lock().unwrap();
        assert_eq!(requests[0].method, "GET");
        assert_eq!(requests[0].url, "/api/v1/private/articles/698");
        assert!(
            requests[0]
                .authorization
                .as_deref()
                .is_some_and(|value| value.starts_with("Basic ")),
            "expected HTTP Basic auth, got {:?}",
            requests[0].authorization
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn get_article_maps_401_to_access_denied() {
        let (addr, _) = spawn_router(|_, _| (401, r#"{"error":"Access denied"}"#.to_string()));

        let error = get_article(&addr, &token(), ArticleId(698))
            .await
            .unwrap_err();

        assert!(
            matches!(error, DevCenterError::AccessDenied),
            "got {error:?}"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn publish_edits_validates_rechecks_saves_and_verifies() {
        let saved = Arc::new(AtomicUsize::new(0));
        let saved_in_router = Arc::clone(&saved);
        let (addr, requests) = spawn_router(move |method, _url| match method {
            "POST" => (200, "{}".to_string()),
            "PUT" => {
                saved_in_router.store(1, Ordering::SeqCst);
                (200, UPDATED.to_string())
            }
            _ if saved_in_router.load(Ordering::SeqCst) == 1 => (200, article_json("old+", 488)),
            _ => (200, article_json("old", 487)),
        });

        let publication = publish_article(&addr, &token(), ArticleId(698), |content| {
            Ok::<_, DevCenterError>(Some(format!("{content}+")))
        })
        .await
        .unwrap();

        assert_eq!(
            publication,
            Publication::Published {
                url: "https://devcenter.heroku.com/articles/ruby-support-reference"
                    .parse()
                    .unwrap(),
                versions_count: 488,
            }
        );
        let requests = requests.lock().unwrap();
        let calls = requests
            .iter()
            .map(|request| format!("{} {}", request.method, request.url))
            .collect::<Vec<_>>();
        assert_eq!(
            calls,
            vec![
                "GET /api/v1/private/articles/698",
                "POST /api/v1/private/articles/698/validate",
                "GET /api/v1/private/articles/698",
                "PUT /api/v1/private/articles/698",
                "GET /api/v1/private/articles/698",
            ]
        );
        assert_eq!(requests[1].body, r#"{"article":{"content":"old+"}}"#);
        assert_eq!(requests[3].body, r#"{"article":{"content":"old+"}}"#);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn publish_writes_nothing_when_the_edit_returns_none() {
        let (addr, requests) = spawn_router(|_, _| (200, article_json("old", 487)));

        let publication = publish_article(&addr, &token(), ArticleId(698), |_| {
            Ok::<_, DevCenterError>(None)
        })
        .await
        .unwrap();

        assert_eq!(
            publication,
            Publication::Unchanged {
                versions_count: 487
            }
        );
        assert_eq!(writes(&requests.lock().unwrap()), Vec::<&str>::new());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn publish_writes_nothing_when_the_edit_refuses_the_article() {
        let (addr, requests) = spawn_router(|_, _| (200, article_json("not ours", 487)));

        let error = publish_article(&addr, &token(), ArticleId(698), |content| {
            Err(DevCenterError::Validation {
                body: format!("unrecognized: {content}"),
            })
        })
        .await
        .unwrap_err();

        assert!(
            matches!(&error, DevCenterError::Validation { body } if body == "unrecognized: not ours"),
            "got {error:?}"
        );
        assert_eq!(writes(&requests.lock().unwrap()), Vec::<&str>::new());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn publish_refuses_when_validation_reports_errors() {
        let (addr, requests) = spawn_router(|method, _url| match method {
            "POST" => (200, r#"{"content":["is too long"]}"#.to_string()),
            _ => (200, article_json("old", 487)),
        });

        let error = publish_article(&addr, &token(), ArticleId(698), replace_with("new"))
            .await
            .unwrap_err();

        match error {
            DevCenterError::ArticleRejected { id, errors } => {
                assert_eq!(id, ArticleId(698));
                assert_eq!(errors.to_string(), "content is too long");
            }
            other => panic!("expected ArticleRejected, got {other:?}"),
        }
        assert_eq!(writes(&requests.lock().unwrap()), vec!["POST"]);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn publish_refuses_when_the_article_changed_since_it_was_read() {
        let gets = Arc::new(AtomicUsize::new(0));
        let gets_in_router = Arc::clone(&gets);
        let (addr, requests) = spawn_router(move |method, _url| match method {
            "POST" => (200, "{}".to_string()),
            _ if gets_in_router.fetch_add(1, Ordering::SeqCst) == 0 => {
                (200, article_json("old", 487))
            }
            _ => (200, article_json("someone else's edit", 488)),
        });

        let error = publish_article(&addr, &token(), ArticleId(698), replace_with("new"))
            .await
            .unwrap_err();

        assert!(
            matches!(
                error,
                DevCenterError::ArticleChanged {
                    id: ArticleId(698),
                    expected: 487,
                    found: 488
                }
            ),
            "got {error:?}"
        );
        assert_eq!(writes(&requests.lock().unwrap()), vec!["POST"]);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn publish_treats_a_200_carrying_model_errors_as_rejected() {
        let (addr, _) = spawn_router(|method, _url| match method {
            "POST" => (200, "{}".to_string()),
            "PUT" => (200, r#"{"content":["can't be blank"]}"#.to_string()),
            _ => (200, article_json("old", 487)),
        });

        let error = publish_article(&addr, &token(), ArticleId(698), replace_with("new"))
            .await
            .unwrap_err();

        match error {
            DevCenterError::ArticleRejected { errors, .. } => {
                assert_eq!(errors.to_string(), "content can't be blank")
            }
            other => panic!("expected ArticleRejected, got {other:?}"),
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn publish_reports_an_unrecognized_save_response_as_unexpected() {
        let (addr, _) = spawn_router(|method, _url| match method {
            "POST" => (200, "{}".to_string()),
            "PUT" => (200, "<html>maintenance</html>".to_string()),
            _ => (200, article_json("old", 487)),
        });

        let error = publish_article(&addr, &token(), ArticleId(698), replace_with("new"))
            .await
            .unwrap_err();

        assert!(
            matches!(&error, DevCenterError::Unexpected { status, body } if *status == StatusCode::OK && body == "<html>maintenance</html>"),
            "got {error:?}"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn publish_errors_when_the_stored_content_differs() {
        let (addr, _) = spawn_router(|method, _url| match method {
            "POST" => (200, "{}".to_string()),
            "PUT" => (200, UPDATED.to_string()),
            _ => (200, article_json("old", 487)),
        });

        let error = publish_article(&addr, &token(), ArticleId(698), replace_with("new"))
            .await
            .unwrap_err();

        assert!(
            matches!(
                error,
                DevCenterError::ArticleNotStored { id: ArticleId(698) }
            ),
            "got {error:?}"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn publish_retries_a_transient_save_error() {
        let puts = Arc::new(AtomicUsize::new(0));
        let puts_in_router = Arc::clone(&puts);
        let (addr, _) = spawn_router(move |method, _url| match method {
            "POST" => (200, "{}".to_string()),
            "PUT" if puts_in_router.fetch_add(1, Ordering::SeqCst) == 0 => {
                (503, "unavailable".to_string())
            }
            "PUT" => (200, UPDATED.to_string()),
            _ if puts_in_router.load(Ordering::SeqCst) > 1 => (200, article_json("new", 488)),
            _ => (200, article_json("old", 487)),
        });

        let publication = publish_article(&addr, &token(), ArticleId(698), replace_with("new"))
            .await
            .unwrap();

        assert!(
            matches!(
                publication,
                Publication::Published {
                    versions_count: 488,
                    ..
                }
            ),
            "got {publication:?}"
        );
        assert_eq!(puts.load(Ordering::SeqCst), 2);
    }
}
