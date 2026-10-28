//! Release data published by ruby-lang.org in the `ruby/www.ruby-lang.org` repository.

use reqwest::Url;
use shared::maybe_err::ResultVec;
use shared::{RubyDownloadVersion, parse_url};
use yaml_rust2::{ScanError, Yaml, YamlLoader};

pub static RELEASES_URL: std::sync::LazyLock<Url> = std::sync::LazyLock::new(|| {
    parse_url("https://api.github.com/repos/ruby/www.ruby-lang.org/contents/_data/releases.yml")
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
}
