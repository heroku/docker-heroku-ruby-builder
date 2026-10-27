//! Generates the Ruby and JRuby version tables in the Dev Center article "Heroku Ruby Support".

use minijinja::{Environment, Value};
use ruby_executable::ruby_lang::RubyBranch;
use shared::RubyDownloadVersion;
use std::fmt;
use std::str::FromStr;
use tokio::task::JoinSet;

pub mod ruby_table;

/// Shown in a cell when nothing is available.
const UNAVAILABLE: &str = "–";

/// A `MAJOR.MINOR` release line, such as Ruby `3.4` or JRuby `10.0`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MajorMinor {
    pub major: u32,
    pub minor: u32,
}

impl fmt::Display for MajorMinor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.major, self.minor)
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("expected a `MAJOR.MINOR` version like `3.4` but got {0:?}")]
pub struct MajorMinorError(String);

impl FromStr for MajorMinor {
    type Err = MajorMinorError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        s.split_once('.')
            .and_then(|(major, minor)| {
                Some(MajorMinor {
                    major: major.parse().ok()?,
                    minor: minor.parse().ok()?,
                })
            })
            .ok_or_else(|| MajorMinorError(s.to_string()))
    }
}

impl From<&RubyDownloadVersion> for MajorMinor {
    fn from(version: &RubyDownloadVersion) -> Self {
        MajorMinor {
            major: version.major,
            minor: version.minor,
        }
    }
}

impl From<&RubyBranch> for MajorMinor {
    fn from(branch: &RubyBranch) -> Self {
        MajorMinor {
            major: branch.major,
            minor: branch.minor,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum CheckError {
    #[error(transparent)]
    S3(#[from] shared::Error),

    #[error("S3 check task failed: {0}")]
    Task(#[from] tokio::task::JoinError),
}

/// Every task's value, or the first error.
async fn join_all<T: 'static>(
    mut set: JoinSet<Result<T, CheckError>>,
) -> Result<Vec<T>, CheckError> {
    let mut values = Vec::new();
    while let Some(joined) = set.join_next().await {
        values.push(joined??);
    }
    Ok(values)
}

/// Renders a table template. Names ending in `.html` escape values for HTML.
fn render(name: &'static str, template: &'static str, context: Value) -> String {
    let mut environment = Environment::new();
    environment
        .add_template(name, template)
        .and_then(|()| environment.get_template(name)?.render(context))
        .expect("table templates render")
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn major_minor_parses() {
        assert_eq!(
            "3.10".parse::<MajorMinor>(),
            Ok(MajorMinor {
                major: 3,
                minor: 10
            })
        );
        for invalid in ["3", "3.x", "3.4.1", ""] {
            assert!(invalid.parse::<MajorMinor>().is_err(), "{invalid:?}");
        }
    }

    #[test]
    fn major_minor_orders_numerically() {
        let older: MajorMinor = "3.9".parse().unwrap();
        let newer: MajorMinor = "3.10".parse().unwrap();
        assert!(older < newer);
    }
}
