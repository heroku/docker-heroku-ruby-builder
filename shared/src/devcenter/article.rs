//! Read and publish Dev Center articles via the private API.
//!
//! `GET /api/v1/private/articles/{id}` returns an article's Markdown `content` and `versions_count`, which increments
//! on every saved edit. `PUT` on the same path saves new content straight to the live article. The private API has
//! no optimistic locking, and a failed save still answers `200` with the model errors as the body.
//!
//! Every write goes through [`publish_article`], which validates the content, refuses to write when the article
//! changed since it was read, and reads the article back to confirm the save.

use super::{DevCenterError, DevCenterToken, client, error_body};
use crate::with_retries_if;
use reqwest::{StatusCode, Url};

/// An article as read from Dev Center.
///
/// Unknown fields in the response are ignored.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub struct Article {
    pub id: u64,
    /// Markdown source. Dev Center stores it with CRLF line endings.
    pub content: String,
    /// Count of saved revisions. A change between two reads means someone else edited the article.
    pub versions_count: u64,
}

/// The result of a successful [`publish_article`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublishedArticle {
    /// Public URL of the article.
    pub url: String,
    /// The article's revision count after the save.
    pub versions_count: u64,
}

#[derive(serde::Serialize)]
struct RequestBody<'a> {
    article: Fields<'a>,
}

#[derive(serde::Serialize)]
struct Fields<'a> {
    content: &'a str,
}

/// A successful update response. A failed save answers with model errors instead, which have no `url`.
#[derive(serde::Deserialize)]
struct UpdatedArticle {
    url: String,
}

fn article_url(host: &Url, id: u64, suffix: &str) -> Url {
    let mut url = host.clone();
    url.set_path(&format!("/api/v1/private/articles/{id}{suffix}"));
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

/// Fetch the article with `id`, retrying transient failures.
///
/// # Errors
///
/// Returns [`DevCenterError`] on transport failure or a non-`200` response.
pub async fn get_article(
    host: &Url,
    token: &DevCenterToken,
    id: u64,
) -> Result<Article, DevCenterError> {
    with_retries_if(DevCenterError::is_retryable, || async {
        let response = client()
            .get(article_url(host, id, ""))
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

/// Ask Dev Center whether `content` would save, without saving it. Valid content answers `{}`.
async fn validate_article(
    host: &Url,
    token: &DevCenterToken,
    id: u64,
    content: &str,
) -> Result<(), DevCenterError> {
    with_retries_if(DevCenterError::is_retryable, || async {
        let response = client()
            .post(article_url(host, id, "/validate"))
            .basic_auth("", Some(token.as_str()))
            .header(reqwest::header::ACCEPT, "application/json")
            .json(&RequestBody {
                article: Fields { content },
            })
            .send()
            .await
            .map_err(DevCenterError::Transport)?;

        if response.status() != StatusCode::OK {
            return Err(status_error(response).await);
        }
        let body = error_body(response).await;
        match serde_json::from_str::<serde_json::Value>(&body) {
            Ok(serde_json::Value::Object(errors)) if errors.is_empty() => Ok(()),
            _ => Err(DevCenterError::ArticleRejected { id, body }),
        }
    })
    .await
}

/// Save `content` to the live article. Private so a write cannot skip the guards in [`publish_article`].
async fn update_article(
    host: &Url,
    token: &DevCenterToken,
    id: u64,
    content: &str,
) -> Result<UpdatedArticle, DevCenterError> {
    with_retries_if(DevCenterError::is_retryable, || async {
        let response = client()
            .put(article_url(host, id, ""))
            .basic_auth("", Some(token.as_str()))
            .header(reqwest::header::ACCEPT, "application/json")
            .json(&RequestBody {
                article: Fields { content },
            })
            .send()
            .await
            .map_err(DevCenterError::Transport)?;

        match response.status() {
            StatusCode::OK => {
                let body = error_body(response).await;
                serde_json::from_str::<UpdatedArticle>(&body)
                    .map_err(|_| DevCenterError::ArticleRejected { id, body })
            }
            StatusCode::UNPROCESSABLE_ENTITY => Err(DevCenterError::ArticleRejected {
                id,
                body: error_body(response).await,
            }),
            _ => Err(status_error(response).await),
        }
    })
    .await
}

/// Replace the content of the article `base` was read from with `content`.
///
/// 1. Validates `content` without saving.
/// 2. Re-reads the article and stops with [`DevCenterError::ArticleChanged`] when its `versions_count` differs from
///    `base`, so a human edit made since `base` was read is not overwritten.
/// 3. Saves `content`.
/// 4. Reads the article back and stops with [`DevCenterError::ArticleNotStored`] unless the stored content equals
///    `content`.
///
/// Without optimistic locking in the API, an edit landing between steps 2 and 3 can still be lost. That window is a
/// single request.
///
/// # Errors
///
/// Returns [`DevCenterError`] when a guard fails, Dev Center rejects the content, or a request fails.
pub async fn publish_article(
    host: &Url,
    token: &DevCenterToken,
    base: &Article,
    content: &str,
) -> Result<PublishedArticle, DevCenterError> {
    validate_article(host, token, base.id, content).await?;

    let current = get_article(host, token, base.id).await?;
    if current.versions_count != base.versions_count {
        return Err(DevCenterError::ArticleChanged {
            id: base.id,
            expected: base.versions_count,
            found: current.versions_count,
        });
    }

    let updated = update_article(host, token, base.id, content).await?;

    let stored = get_article(host, token, base.id).await?;
    if stored.content != content {
        return Err(DevCenterError::ArticleNotStored { id: base.id });
    }

    Ok(PublishedArticle {
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

    fn base(content: &str, versions_count: u64) -> Article {
        Article {
            id: 698,
            content: content.to_string(),
            versions_count,
        }
    }

    const UPDATED: &str = r#"{"title":"Heroku Ruby Support","status":"published","url":"https://devcenter.heroku.com/articles/ruby-support-reference","owner_email":"owner@example.com"}"#;

    #[tokio::test(flavor = "multi_thread")]
    async fn get_article_reads_content_and_versions_count() {
        let (addr, requests) = spawn_router(|_, _| (200, article_json("a\r\nb", 487)));

        let article = get_article(&addr, &token(), 698).await.unwrap();

        assert_eq!(article, base("a\r\nb", 487));
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

        let error = get_article(&addr, &token(), 698).await.unwrap_err();

        assert!(
            matches!(error, DevCenterError::AccessDenied),
            "got {error:?}"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn publish_validates_rechecks_saves_and_verifies() {
        let saved = Arc::new(AtomicUsize::new(0));
        let saved_in_router = Arc::clone(&saved);
        let (addr, requests) = spawn_router(move |method, _url| match method {
            "POST" => (200, "{}".to_string()),
            "PUT" => {
                saved_in_router.store(1, Ordering::SeqCst);
                (200, UPDATED.to_string())
            }
            _ if saved_in_router.load(Ordering::SeqCst) == 1 => (200, article_json("new", 488)),
            _ => (200, article_json("old", 487)),
        });

        let published = publish_article(&addr, &token(), &base("old", 487), "new")
            .await
            .unwrap();

        assert_eq!(
            published,
            PublishedArticle {
                url: "https://devcenter.heroku.com/articles/ruby-support-reference".to_string(),
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
                "POST /api/v1/private/articles/698/validate",
                "GET /api/v1/private/articles/698",
                "PUT /api/v1/private/articles/698",
                "GET /api/v1/private/articles/698",
            ]
        );
        assert_eq!(requests[0].body, r#"{"article":{"content":"new"}}"#);
        assert_eq!(requests[2].body, r#"{"article":{"content":"new"}}"#);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn publish_refuses_when_validation_reports_errors() {
        let (addr, requests) = spawn_router(|method, _url| match method {
            "POST" => (200, r#"{"content":["is too long"]}"#.to_string()),
            _ => (200, article_json("old", 487)),
        });

        let error = publish_article(&addr, &token(), &base("old", 487), "new")
            .await
            .unwrap_err();

        match error {
            DevCenterError::ArticleRejected { id, body } => {
                assert_eq!(id, 698);
                assert!(body.contains("is too long"), "got {body}");
            }
            other => panic!("expected ArticleRejected, got {other:?}"),
        }
        assert!(
            requests.lock().unwrap().iter().all(|r| r.method != "PUT"),
            "must not save content that failed validation"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn publish_refuses_when_the_article_changed_since_it_was_read() {
        let (addr, requests) = spawn_router(|method, _url| match method {
            "POST" => (200, "{}".to_string()),
            _ => (200, article_json("someone else's edit", 488)),
        });

        let error = publish_article(&addr, &token(), &base("old", 487), "new")
            .await
            .unwrap_err();

        assert!(
            matches!(
                error,
                DevCenterError::ArticleChanged {
                    id: 698,
                    expected: 487,
                    found: 488
                }
            ),
            "got {error:?}"
        );
        assert!(
            requests.lock().unwrap().iter().all(|r| r.method != "PUT"),
            "must not overwrite a concurrent edit"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn publish_treats_a_200_carrying_model_errors_as_rejected() {
        let (addr, _) = spawn_router(|method, _url| match method {
            "POST" => (200, "{}".to_string()),
            "PUT" => (200, r#"{"content":["can't be blank"]}"#.to_string()),
            _ => (200, article_json("old", 487)),
        });

        let error = publish_article(&addr, &token(), &base("old", 487), "new")
            .await
            .unwrap_err();

        match error {
            DevCenterError::ArticleRejected { body, .. } => {
                assert!(body.contains("can't be blank"), "got {body}")
            }
            other => panic!("expected ArticleRejected, got {other:?}"),
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn publish_errors_when_the_stored_content_differs() {
        let (addr, _) = spawn_router(|method, _url| match method {
            "POST" => (200, "{}".to_string()),
            "PUT" => (200, UPDATED.to_string()),
            _ => (200, article_json("old", 487)),
        });

        let error = publish_article(&addr, &token(), &base("old", 487), "new")
            .await
            .unwrap_err();

        assert!(
            matches!(error, DevCenterError::ArticleNotStored { id: 698 }),
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

        let published = publish_article(&addr, &token(), &base("old", 487), "new")
            .await
            .unwrap();

        assert_eq!(published.versions_count, 488);
        assert_eq!(puts.load(Ordering::SeqCst), 2);
    }
}
