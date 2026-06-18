use reqwest::StatusCode;
use std::time::Duration;
use url::Url;

/// Check if a given S3 URL exists or not
pub async fn url_exists(url: Url) -> Result<bool, Error> {
    crate::with_retries(|| s3_url_exists_inner(url.clone())).await
}

/// Blocking wrappers around the async S3 helpers for use from synchronous code.
pub mod sync {
    use super::*;

    /// Blocking version of [`super::url_exists`].
    pub fn url_exists(url: Url) -> Result<bool, Error> {
        tokio::runtime::Runtime::new()?.block_on(super::url_exists(url))
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Http(#[from] reqwest::Error),

    #[error("Could not start async runtime: {0}")]
    Io(#[from] std::io::Error),

    #[error("Unexpected status {status} checking {url}")]
    UnexpectedStatus { url: Url, status: StatusCode },
}

async fn s3_url_exists_inner(url: Url) -> Result<bool, Error> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()?;
    let response = client.head(url.clone()).send().await?;
    match response.status() {
        status if status.is_success() => Ok(true),
        reqwest::StatusCode::NOT_FOUND | reqwest::StatusCode::FORBIDDEN => Ok(false),
        status => Err(Error::UnexpectedStatus { url, status }),
    }
}
