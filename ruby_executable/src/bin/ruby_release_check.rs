use bullet_stream::global::print;
use clap::Parser;
use fs_err as fs;
use reqwest::{Client, Url};
use shared::maybe_err::{MaybeErrors, NonEmptyErrors, OkMaybe};
use shared::{RubyDownloadVersion, S3_BASE_URL, build_matrix, output_ruby_tar_path};
use std::{
    error::Error,
    path::{Path, PathBuf},
    time::Duration,
};
use tokio::task::JoinSet;
use yaml_rust2::{ScanError, Yaml, YamlLoader};

static RELEASES_URL: std::sync::LazyLock<Url> = std::sync::LazyLock::new(|| {
    Url::parse("https://raw.githubusercontent.com/ruby/www.ruby-lang.org/master/_data/releases.yml")
        .expect("valid releases URL constant")
});

#[derive(Parser, Debug)]
#[command(about = "Check for Ruby releases missing from Heroku S3")]
struct Args {
    /// Minimum Ruby version to check (e.g. 3.2.0). All releases >= this version will be checked.
    #[arg(long = "minimum-version", required = true)]
    minimum_version: RubyDownloadVersion,

    /// Path to write JSON output file containing versions that need builds
    #[arg(long = "output", required = true)]
    output: PathBuf,
}

async fn get_body(client: &Client, url: Url) -> Result<String, reqwest::Error> {
    client
        .get(url)
        .send()
        .await?
        .error_for_status()?
        .text()
        .await
}

async fn fetch_ruby_lang_body(url: &Url) -> Result<String, reqwest::Error> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()?;

    shared::with_retries(|| get_body(&client, url.clone())).await
}

#[derive(Debug, thiserror::Error)]
enum FlatYamlError {
    #[error("Cannot parse yaml due to error {1} from input:\n{0}")]
    NotYaml(String, ScanError),
    #[error("Expected first yaml element to be a vec but it was not: {1:?} from input:\n{0}")]
    FirstNotVec(String, Vec<Yaml>),
}

#[derive(Debug, thiserror::Error)]
enum RubyLangEntryError {
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
fn ruby_lang_versions(
    body: String,
) -> OkMaybe<Vec<RubyDownloadVersion>, NonEmptyErrors<RubyLangEntryError>> {
    let mut errors = MaybeErrors::new();
    let mut releases = Vec::new();

    match parse_flat_yaml(body) {
        Ok(entries) => {
            for entry in entries {
                match entry["version"]
                    .as_str()
                    .ok_or_else(|| RubyLangEntryError::MissingVersion(entry.clone()))
                    .and_then(|v| {
                        RubyDownloadVersion::new(v).map_err(RubyLangEntryError::CannotParse)
                    }) {
                    Ok(v) => releases.push(v),
                    Err(error) => errors.push(error),
                }
            }
        }
        Err(error) => {
            errors.push(error.into());
        }
    }
    errors.ok_maybe(releases)
}

fn version_gte(version: &RubyDownloadVersion, minimum: &RubyDownloadVersion) -> bool {
    let version_tuple = (version.major, version.minor, version.patch);
    let minimum_tuple = (minimum.major, minimum.minor, minimum.patch);
    version_tuple >= minimum_tuple
}

fn retain_releases_gte(
    releases: &[RubyDownloadVersion],
    minimum: &RubyDownloadVersion,
) -> Vec<RubyDownloadVersion> {
    releases
        .iter()
        .filter(|version| version_gte(version, minimum))
        .cloned()
        .collect()
}

fn urls_to_check(version: &RubyDownloadVersion) -> Vec<(String, Url)> {
    let matrix = build_matrix();
    let base_url = Url::parse(S3_BASE_URL).expect("valid base URL constant");
    matrix
        .iter()
        .map(|(base_image, arch)| {
            let tar_path = output_ruby_tar_path(Path::new(""), version, base_image, Some(arch));
            let mut url = base_url.clone();
            url.path_segments_mut()
                .expect("valid base URL")
                .extend(tar_path.iter().map(|s| s.to_string_lossy()));
            (format!("{base_image}/{arch}"), url)
        })
        .collect()
}

async fn s3_url_exists(url: Url) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
    shared::with_retries(|| s3_url_exists_inner(url.clone())).await
}

async fn s3_url_exists_inner(url: Url) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()?;
    let response = client.head(url.clone()).send().await?;
    match response.status() {
        status if status.is_success() => Ok(true),
        reqwest::StatusCode::NOT_FOUND | reqwest::StatusCode::FORBIDDEN => Ok(false),
        status => Err(format!("Unexpected status {status} checking {url}").into()),
    }
}

async fn check_version_on_s3(
    version: RubyDownloadVersion,
) -> Result<(RubyDownloadVersion, Vec<String>), Box<dyn std::error::Error + Send + Sync>> {
    let mut set = JoinSet::new();
    for (label, url) in urls_to_check(&version) {
        set.spawn(async move {
            let exists = s3_url_exists(url).await?;
            Ok::<_, Box<dyn std::error::Error + Send + Sync>>((label, exists))
        });
    }

    let mut missing = Vec::new();
    while let Some(result) = set.join_next().await {
        let (label, exists) = result??;
        if !exists {
            missing.push(label);
        }
    }

    Ok((version, missing))
}

async fn call(args: Args) -> OkMaybe<(), NonEmptyErrors<Box<dyn Error>>> {
    print::h2("Checking for new Ruby releases");
    print::bullet(format!("Minimum version: {}", args.minimum_version));

    let mut errors: MaybeErrors<Box<dyn Error>> = MaybeErrors::new();

    print::h2(format!("Fetching releases from {}", *RELEASES_URL));
    let releases = match fetch_ruby_lang_body(&RELEASES_URL).await {
        Ok(body) => ruby_lang_versions(body).drain_unwrap(&mut errors),
        Err(e) => {
            errors.push(e.into());
            Vec::new()
        }
    };
    print::bullet(format!("Found {} total releases", releases.len()));

    let versions_to_check = retain_releases_gte(&releases, &args.minimum_version);

    print::bullet(format!(
        "Checking {} versions on S3",
        versions_to_check.len()
    ));

    let mut set = JoinSet::new();
    for version in versions_to_check {
        set.spawn(check_version_on_s3(version));
    }

    let mut versions_to_build = Vec::new();
    while let Some(result) = set.join_next().await {
        match result.map_err(|e| e.into()) {
            Ok(Ok((version, missing))) => {
                if missing.is_empty() {
                    print::sub_bullet(format!("{version}: all binaries present"));
                } else {
                    print::sub_bullet(format!(
                        "{version}: missing {} combo(s): {}",
                        missing.len(),
                        missing.join(", ")
                    ));
                    versions_to_build.push(version);
                }
            }
            Err(e) | Ok(Err(e)) => errors.push(e),
        }
    }

    if let Err(error) = serde_json::to_string_pretty(&versions_to_build)
        .map_err(|e| e.into())
        .and_then(|json| fs::write(&args.output, &json).map_err(|e| Box::new(e) as Box<dyn Error>))
    {
        errors.push(error)
    };

    if versions_to_build.is_empty() {
        print::bullet("No versions to build found");
    } else {
        print::h2("Versions needing builds");
        for version in &versions_to_build {
            print::sub_bullet(format!("{version}"));
        }
    }
    errors.ok_maybe(())
}

#[tokio::main]
async fn main() {
    let args = Args::parse();
    match call(args).await {
        OkMaybe(_, None) => print::bullet("Done"),
        OkMaybe(_, Some(errors)) => {
            print::error(format!("Failed {errors}"));
            std::process::exit(1);
        }
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

        let mut errors = MaybeErrors::<RubyLangEntryError>::new();
        assert_eq!(
            vec![String::from("4.0.5")],
            ruby_lang_versions(body)
                .drain_unwrap(&mut errors)
                .iter()
                .map(|v| v.to_string())
                .collect::<Vec<_>>()
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

        let mut errors = MaybeErrors::<RubyLangEntryError>::new();
        assert_eq!(
            vec![String::from("4.0.5")],
            ruby_lang_versions(body)
                .drain_unwrap(&mut errors)
                .iter()
                .map(|v| v.to_string())
                .collect::<Vec<_>>()
        );

        assert_eq!(1, errors.len());
        assert_matches!(
            errors.into_iter().next().unwrap(),
            RubyLangEntryError::MissingVersion(_)
        );
    }

    #[test]
    fn test_version_gte() {
        let min = RubyDownloadVersion::new("3.2.0").unwrap();
        assert!(version_gte(
            &RubyDownloadVersion::new("3.2.0").unwrap(),
            &min
        ));
        assert!(version_gte(
            &RubyDownloadVersion::new("3.3.7").unwrap(),
            &min
        ));
        assert!(version_gte(
            &RubyDownloadVersion::new("4.0.0").unwrap(),
            &min
        ));
        assert!(!version_gte(
            &RubyDownloadVersion::new("3.1.9").unwrap(),
            &min
        ));
        assert!(!version_gte(
            &RubyDownloadVersion::new("2.7.8").unwrap(),
            &min
        ));
    }

    #[test]
    fn test_version_gte_prerelease() {
        let min = RubyDownloadVersion::new("3.4.0").unwrap();
        assert!(version_gte(
            &RubyDownloadVersion::new("3.4.0-preview1").unwrap(),
            &min
        ));
        assert!(!version_gte(
            &RubyDownloadVersion::new("3.3.9").unwrap(),
            &min
        ));
    }

    #[test]
    fn test_retain_releases_gte() {
        let releases = vec![
            RubyDownloadVersion::new("3.4.1").unwrap(),
            RubyDownloadVersion::new("3.3.7").unwrap(),
            RubyDownloadVersion::new("3.2.0").unwrap(),
            RubyDownloadVersion::new("3.1.5").unwrap(),
            RubyDownloadVersion::new("2.7.8").unwrap(),
        ];
        let min = RubyDownloadVersion::new("3.2.0").unwrap();
        let filtered = retain_releases_gte(&releases, &min);
        let names: Vec<String> = filtered.iter().map(|v| v.to_string()).collect();
        assert_eq!(vec!["3.4.1", "3.3.7", "3.2.0"], names);
    }
}
