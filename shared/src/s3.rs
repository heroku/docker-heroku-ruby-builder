use reqwest::StatusCode;
use std::time::Duration;
use tokio::time::sleep;
use url::Url;

const MAX_RETRY_ATTEMPTS: u8 = 3;
const RETRY_DELAY: Duration = Duration::from_secs(1);

/// Check if a given S3 URL exists or not
pub async fn url_exists(url: Url) -> Result<bool, Error> {
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

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Http(#[from] reqwest::Error),

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
