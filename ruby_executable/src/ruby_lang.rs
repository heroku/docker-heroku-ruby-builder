//! Release data published by ruby-lang.org in the `ruby/www.ruby-lang.org` repository.

use reqwest::Url;
use shared::maybe_err::ResultVec;
use shared::{MajorMinor, RubyDownloadVersion, parse_url};
use yaml_rust2::{ScanError, Yaml, YamlLoader};

pub static RELEASES_URL: std::sync::LazyLock<Url> = std::sync::LazyLock::new(|| {
    parse_url("https://api.github.com/repos/ruby/www.ruby-lang.org/contents/_data/releases.yml")
        .expect("internal literal should be a valid URL")
});

pub static BRANCHES_URL: std::sync::LazyLock<Url> = std::sync::LazyLock::new(|| {
    parse_url("https://api.github.com/repos/ruby/www.ruby-lang.org/contents/_data/branches.yml")
        .expect("internal literal should be a valid URL")
});

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

#[derive(Debug, Clone, Eq, PartialEq, thiserror::Error)]
#[error("Unknown branch status `{0}`")]
pub struct UnknownBranchStatus(String);

impl BranchStatus {
    fn from_status(input: &str) -> Result<BranchStatus, UnknownBranchStatus> {
        match input {
            "normal maintenance" => Ok(BranchStatus::NormalMaintenance),
            "security maintenance" => Ok(BranchStatus::SecurityMaintenance),
            "eol" => Ok(BranchStatus::Eol),
            "preview" => Ok(BranchStatus::Preview),
            _ => Err(UnknownBranchStatus(input.to_string())),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum BranchEntryError {
    #[error(transparent)]
    DocError(#[from] FlatYamlError),

    #[error("expected a `name` field like `3.4` but got: {0:?}")]
    InvalidName(Yaml),

    #[error("Ruby branch {branch}: {status}\n{document:?}")]
    UnknownStatus {
        branch: MajorMinor,
        status: UnknownBranchStatus,
        document: Yaml,
    },
}

/// Parses each branch's release line and status from <https://raw.githubusercontent.com/ruby/www.ruby-lang.org/master/_data/branches.yml>
pub fn ruby_lang_branches(
    body: String,
) -> Result<Vec<(MajorMinor, BranchStatus)>, BranchEntryError> {
    parse_flat_yaml(body)?
        .iter()
        .map(|entry| {
            let branch = major_minor(&entry["name"])
                .ok_or_else(|| BranchEntryError::InvalidName(entry.clone()))?;
            let status = BranchStatus::from_status(entry["status"].as_str().unwrap_or(""))
                .map_err(
                    |status: UnknownBranchStatus| BranchEntryError::UnknownStatus {
                        branch,
                        status,
                        document: entry.clone(),
                    },
                )?;
            Ok((branch, status))
        })
        .collect()
}

/// Reads `MAJOR.MINOR` from a branch name with optional trailing `PATCH`.
fn major_minor(name: &Yaml) -> Option<MajorMinor> {
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
        [major, minor] | [major, minor, _] => Some(MajorMinor { major, minor }),
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
                (MajorMinor { major: 4, minor: 1 }, BranchStatus::Preview),
                (
                    MajorMinor { major: 4, minor: 0 },
                    BranchStatus::NormalMaintenance
                ),
                (
                    MajorMinor { major: 3, minor: 3 },
                    BranchStatus::SecurityMaintenance
                ),
                (
                    MajorMinor {
                        major: 3,
                        minor: 10
                    },
                    BranchStatus::Eol
                ),
                (MajorMinor { major: 1, minor: 9 }, BranchStatus::Eol),
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
                branch: MajorMinor { major: 3, minor: 3 },
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
