//! Generates the Ruby and JRuby version tables in the Dev Center article "Heroku Ruby Support".

use libherokubuildpack::inventory::artifact::Arch;
use shared::{BaseImage, S3_BASE_URL, build_matrix};
use std::fmt;
use std::path::Path;
use std::str::FromStr;
use url::Url;

pub mod guard;
pub mod html_table;
pub mod jruby_table;
pub mod markers;
pub mod ruby_table;
pub mod update;

/// Dev Center id of <https://devcenter.heroku.com/articles/ruby-support-reference>.
pub const ARTICLE_ID: u64 = 698;

/// Shown in a cell when nothing is available.
pub const UNAVAILABLE: &str = "–";

/// A `MAJOR.MINOR` release line, such as Ruby `3.4` or JRuby `10.0`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Series {
    pub major: u32,
    pub minor: u32,
}

impl fmt::Display for Series {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.major, self.minor)
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("expected a `MAJOR.MINOR` series like `3.4` but got {0:?}")]
pub struct SeriesError(String);

impl FromStr for Series {
    type Err = SeriesError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        s.split_once('.')
            .and_then(|(major, minor)| {
                Some(Series {
                    major: major.parse().ok()?,
                    minor: minor.parse().ok()?,
                })
            })
            .ok_or_else(|| SeriesError(s.to_string()))
    }
}

/// A base image and every architecture it is built for. A version counts as available on a stack only when it
/// exists for all of them.
#[derive(Debug, Clone)]
pub struct Stack {
    pub base_image: BaseImage,
    pub archs: Vec<Arch>,
}

/// Every stack in `build_matrix()` order.
pub fn stacks() -> Vec<Stack> {
    let mut stacks: Vec<Stack> = Vec::new();
    for (base_image, arch) in build_matrix() {
        match stacks.last_mut() {
            Some(stack) if stack.base_image.name() == base_image.name() => stack.archs.push(arch),
            _ => stacks.push(Stack {
                base_image,
                archs: vec![arch],
            }),
        }
    }
    stacks
}

/// The S3 URL of an object at `path` relative to the bucket root.
fn s3_url(path: &Path) -> Url {
    let mut url = Url::parse(S3_BASE_URL).expect("valid base URL constant");
    url.path_segments_mut()
        .expect("valid base URL")
        .extend(path.iter().map(|segment| segment.to_string_lossy()));
    url
}

/// Whether every URL exists, checked in order and stopping at the first missing one.
async fn all_exist<F, Fut, E>(urls: Vec<Url>, exists: &F) -> Result<bool, E>
where
    F: Fn(Url) -> Fut,
    Fut: Future<Output = Result<bool, E>>,
{
    for url in urls {
        if !exists(url).await? {
            return Ok(false);
        }
    }
    Ok(true)
}

#[cfg(test)]
pub(crate) mod test_support {
    use std::collections::HashSet;
    use url::Url;

    /// An S3 stand-in that holds exactly the given object paths.
    pub(crate) fn bucket(
        paths: &[&str],
    ) -> impl Fn(Url) -> std::future::Ready<Result<bool, String>> {
        let paths = paths
            .iter()
            .map(|path| path.to_string())
            .collect::<HashSet<_>>();
        move |url: Url| std::future::ready(Ok(paths.contains(url.path().trim_start_matches('/'))))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn series_parses_major_minor() {
        assert_eq!(
            "3.10".parse::<Series>(),
            Ok(Series {
                major: 3,
                minor: 10
            })
        );
        for invalid in ["3", "3.x", "3.4.1", ""] {
            assert!(invalid.parse::<Series>().is_err(), "{invalid:?}");
        }
    }

    #[test]
    fn series_orders_numerically() {
        let older: Series = "3.9".parse().unwrap();
        let newer: Series = "3.10".parse().unwrap();
        assert!(older < newer);
    }

    #[test]
    fn stacks_group_architectures_by_base_image() {
        let stacks = stacks()
            .iter()
            .map(|stack| {
                format!(
                    "{}: {}",
                    stack.base_image,
                    stack
                        .archs
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join(",")
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(
            stacks,
            vec![
                "heroku-22: amd64",
                "heroku-24: amd64,arm64",
                "heroku-26: amd64,arm64"
            ]
        );
    }
}
