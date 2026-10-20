//! Release data published by ruby-lang.org in the `ruby/www.ruby-lang.org` repository.

use reqwest::{Client, Url};
use shared::RubyDownloadVersion;
use shared::maybe_err::ResultVec;
use std::time::Duration;
use yaml_rust2::{ScanError, Yaml, YamlLoader};

pub static RELEASES_URL: std::sync::LazyLock<Url> = std::sync::LazyLock::new(|| {
    Url::parse("https://raw.githubusercontent.com/ruby/www.ruby-lang.org/master/_data/releases.yml")
        .expect("valid releases URL constant")
});

pub static BRANCHES_URL: std::sync::LazyLock<Url> = std::sync::LazyLock::new(|| {
    Url::parse("https://raw.githubusercontent.com/ruby/www.ruby-lang.org/master/_data/branches.yml")
        .expect("valid branches URL constant")
});

async fn get_body(client: &Client, url: Url) -> Result<String, reqwest::Error> {
    client
        .get(url)
        .send()
        .await?
        .error_for_status()?
        .text()
        .await
}

pub async fn fetch_ruby_lang_body(url: &Url) -> Result<String, reqwest::Error> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()?;

    shared::with_retries(|| get_body(&client, url.clone())).await
}

#[derive(Debug, thiserror::Error)]
pub enum FlatYamlError {
    #[error("Cannot parse yaml due to error {1} from input:\n{0}")]
    NotYaml(String, ScanError),
    #[error("Expected first yaml element to be a vec but it was not: {1:?} from input:\n{0}")]
    FirstNotVec(String, Vec<Yaml>),
}

#[derive(Debug, thiserror::Error)]
pub enum RubyLangEntryError {
    #[error(transparent)]
    DocError(#[from] FlatYamlError),

    #[error("expected yaml to have a `version` field but it did not: {0:?}")]
    MissingVersion(Yaml),

    #[error(transparent)]
    CannotParse(#[from] shared::Error),
}

/// Parse output from <https://raw.githubusercontent.com/ruby/www.ruby-lang.org/master/_data/releases.yml>
fn parse_flat_yaml(body: String) -> Result<Vec<Yaml>, FlatYamlError> {
    YamlLoader::load_from_str(&body)
        .map_err(|error| FlatYamlError::NotYaml(body.clone(), error))
        .and_then(|docs| {
            docs.first()
                .and_then(|doc| doc.as_vec())
                .cloned()
                .ok_or(FlatYamlError::FirstNotVec(body.clone(), docs.clone()))
        })
}

/// Parses output from Ruby Lang into Ruby Versions
///
/// Fault tolerant parse result of <https://raw.githubusercontent.com/ruby/www.ruby-lang.org/master/_data/releases.yml>
pub fn ruby_lang_versions(body: String) -> ResultVec<RubyDownloadVersion, RubyLangEntryError> {
    match parse_flat_yaml(body) {
        Ok(entries) => entries
            .into_iter()
            .map(|entry| {
                entry["version"]
                    .as_str()
                    .ok_or_else(|| RubyLangEntryError::MissingVersion(entry.clone()))
                    .and_then(|v| {
                        RubyDownloadVersion::new(v).map_err(RubyLangEntryError::CannotParse)
                    })
            })
            .collect(),
        Err(error) => vec![Err(error.into())],
    }
    .into()
}

/// Upstream maintenance status of a Ruby branch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BranchStatus {
    NormalMaintenance,
    SecurityMaintenance,
    Eol,
    Preview,
}

/// One entry of `branches.yml`, such as Ruby `3.3` in security maintenance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RubyBranch {
    pub major: u32,
    pub minor: u32,
    pub status: BranchStatus,
}

#[derive(Debug, thiserror::Error)]
pub enum BranchEntryError {
    #[error(transparent)]
    DocError(#[from] FlatYamlError),

    #[error("expected a `name` field like `3.4` but got: {0:?}")]
    InvalidName(Yaml),

    #[error("unknown `status` for Ruby branch {major}.{minor}: {status:?}")]
    UnknownStatus {
        major: u32,
        minor: u32,
        status: Yaml,
    },
}

/// Parses <https://raw.githubusercontent.com/ruby/www.ruby-lang.org/master/_data/branches.yml>
///
/// Any malformed entry fails the whole parse, so an unexpected status is never mistaken for a known one.
pub fn ruby_lang_branches(body: String) -> Result<Vec<RubyBranch>, BranchEntryError> {
    parse_flat_yaml(body)?
        .iter()
        .map(|entry| {
            let (major, minor) = branch_name(&entry["name"])
                .ok_or_else(|| BranchEntryError::InvalidName(entry.clone()))?;
            let status = match entry["status"].as_str() {
                Some("normal maintenance") => BranchStatus::NormalMaintenance,
                Some("security maintenance") => BranchStatus::SecurityMaintenance,
                Some("eol") => BranchStatus::Eol,
                Some("preview") => BranchStatus::Preview,
                _ => {
                    return Err(BranchEntryError::UnknownStatus {
                        major,
                        minor,
                        status: entry["status"].clone(),
                    });
                }
            };
            Ok(RubyBranch {
                major,
                minor,
                status,
            })
        })
        .collect()
}

/// Reads `MAJOR.MINOR` from a branch name. YAML loads an unquoted `4.0` as a float, and the oldest branches carry a
/// patch segment (`1.9.3`), so both are accepted.
fn branch_name(name: &Yaml) -> Option<(u32, u32)> {
    let name = match name {
        Yaml::Real(name) | Yaml::String(name) => name,
        _ => return None,
    };
    let segments = name
        .split('.')
        .map(str::parse::<u32>)
        .collect::<Result<Vec<_>, _>>()
        .ok()?;
    match segments[..] {
        [major, minor] | [major, minor, _] => Some((major, minor)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::assert_matches;

    #[test]
    fn ruby_lang_parsing_returns_partial_result_on_parse_failure() {
        let body = indoc::indoc! {"
            - version: 4.0.5
            - version: 4.doesnotparse.5
        "}
        .to_string();

        let mut errors = Vec::new();
        let versions = ruby_lang_versions(body).unwrap_drain_errs(&mut errors);
        assert_eq!(
            vec![String::from("4.0.5")],
            versions.iter().map(|v| v.to_string()).collect::<Vec<_>>()
        );

        assert_eq!(1, errors.len());
        assert_matches!(
            errors.into_iter().next().unwrap(),
            RubyLangEntryError::CannotParse(_)
        );
    }

    #[test]
    fn parse_flat_yaml_errors_on_unparseable_yaml() {
        let body = String::from("cannot_parse: 'unterminated_string");
        assert_matches!(parse_flat_yaml(body), Err(FlatYamlError::NotYaml(_, _)));
    }

    #[test]
    fn parse_flat_yaml_errors_when_top_level_not_vec() {
        let body = String::from("version: 4.0.5");
        assert_matches!(parse_flat_yaml(body), Err(FlatYamlError::FirstNotVec(_, _)));
    }

    #[test]
    fn ruby_lang_versions_errors_on_missing_version_field() {
        let body = indoc::indoc! {"
            - name: ruby
            - version: 4.0.5
        "}
        .to_string();

        let mut errors = Vec::new();
        let versions = ruby_lang_versions(body).unwrap_drain_errs(&mut errors);
        assert_eq!(
            vec![String::from("4.0.5")],
            versions.iter().map(|v| v.to_string()).collect::<Vec<_>>()
        );

        assert_eq!(1, errors.len());
        assert_matches!(
            errors.into_iter().next().unwrap(),
            RubyLangEntryError::MissingVersion(_)
        );
    }

    #[test]
    fn ruby_lang_branches_parses_names_and_statuses() {
        let body = indoc::indoc! {"
            - name: 4.1
              status: preview
            - name: 4.0
              status: normal maintenance
            - name: 3.3
              status: security maintenance
            - name: \"3.10\"
              status: eol
            - name: 1.9.3
              status: eol
        "}
        .to_string();

        assert_eq!(
            ruby_lang_branches(body).unwrap(),
            vec![
                RubyBranch {
                    major: 4,
                    minor: 1,
                    status: BranchStatus::Preview
                },
                RubyBranch {
                    major: 4,
                    minor: 0,
                    status: BranchStatus::NormalMaintenance
                },
                RubyBranch {
                    major: 3,
                    minor: 3,
                    status: BranchStatus::SecurityMaintenance
                },
                RubyBranch {
                    major: 3,
                    minor: 10,
                    status: BranchStatus::Eol
                },
                RubyBranch {
                    major: 1,
                    minor: 9,
                    status: BranchStatus::Eol
                },
            ]
        );
    }

    #[test]
    fn ruby_lang_branches_errors_on_unknown_status() {
        let body = indoc::indoc! {"
            - name: 3.4
              status: normal maintenance
            - name: 3.3
              status: extended maintenance
        "}
        .to_string();

        assert_matches!(
            ruby_lang_branches(body),
            Err(BranchEntryError::UnknownStatus {
                major: 3,
                minor: 3,
                ..
            })
        );
    }

    #[test]
    fn ruby_lang_branches_errors_on_invalid_name() {
        for name in ["three", "3", "3.4.1.0"] {
            let body = format!("- name: {name}\n  status: eol\n");
            assert_matches!(
                ruby_lang_branches(body),
                Err(BranchEntryError::InvalidName(_)),
                "name {name:?}"
            );
        }
    }
}
