use bullet_stream::global::print;
use clap::Parser;
use fs_err as fs;
use libherokubuildpack::inventory::artifact::Arch;
use reqwest::Url;
use ruby_executable::ruby_lang::{RELEASES_URL, ruby_lang_versions};
use shared::github::{self, GitHubTokenArg};
use shared::maybe_err::ResultVec;
use shared::{
    BaseImage, RubyDownloadVersion, build_matrix, output_ruby_tar_path, s3_presence, s3_url,
};
use std::{
    error::Error,
    path::{Path, PathBuf},
};
use tokio::task::JoinSet;
#[derive(Parser, Debug)]
#[command(about = "Check for Ruby releases missing from Heroku S3")]
struct Args {
    #[command(flatten)]
    github: GitHubTokenArg,

    /// Minimum Ruby version to check (e.g. 3.2.0). All releases >= this version will be checked.
    #[arg(long = "minimum-version", required = true)]
    minimum_version: RubyDownloadVersion,

    /// Path to write JSON output file containing versions that need builds
    #[arg(long = "output", required = true)]
    output: PathBuf,
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

fn urls_to_check(version: &RubyDownloadVersion) -> Vec<(Url, BaseImage, Arch)> {
    build_matrix()
        .into_iter()
        .map(|(base_image, arch)| {
            let tar_path = output_ruby_tar_path(Path::new(""), version, &base_image, Some(&arch));
            (s3_url(&tar_path), base_image, arch)
        })
        .collect()
}

async fn check_version_on_s3(
    version: RubyDownloadVersion,
) -> Result<(RubyDownloadVersion, Vec<String>), shared::Error> {
    let missing = s3_presence(urls_to_check(&version))
        .await?
        .missing
        .into_iter()
        .map(|(base_image, arch)| format!("{base_image}/{arch}"))
        .collect();
    Ok((version, missing))
}

async fn call(args: Args) -> ResultVec<(), Box<dyn Error>> {
    print::h2("Checking for new Ruby releases");
    print::bullet(format!("Minimum version: {}", args.minimum_version));

    let mut errors: Vec<Box<dyn Error>> = Vec::new();
    print::h2(format!("Fetching releases from {}", *RELEASES_URL));
    let releases = match github::get_raw_with_retry(&RELEASES_URL, &args.github.token).await {
        Ok(response) => ruby_lang_versions(response.body).unwrap_drain_errs(&mut errors),
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
        match result {
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
            Ok(Err(e)) => errors.push(e.into()),
            Err(e) => errors.push(e.into()),
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
    errors.into_iter().map(Result::Err).collect()
}

#[tokio::main]
async fn main() {
    let args = Args::parse();
    let errors: Vec<_> = call(args)
        .await
        .into_iter()
        .filter_map(Result::err)
        .collect();

    if errors.is_empty() {
        print::bullet("done");
    } else {
        let errors = errors
            .into_iter()
            .map(|error| format!("- {error}"))
            .collect::<Vec<_>>()
            .join("\n");
        print::error(format!("Failed\n\n{errors}"));
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

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

    #[test]
    fn get_help() {
        insta::assert_snapshot!(shared::help(Args::command(), &[]), @r"
        Check for Ruby releases missing from Heroku S3

        Usage: ruby_release_check --gh-token <TOKEN> --minimum-version <MINIMUM_VERSION> --output <OUTPUT>

        Options:
              --gh-token <TOKEN>
                  GitHub API token. e.g. `--gh-token=$(gh auth token)` [env: GITHUB_TOKEN]
              --minimum-version <MINIMUM_VERSION>
                  Minimum Ruby version to check (e.g. 3.2.0). All releases >= this version will be checked
              --output <OUTPUT>
                  Path to write JSON output file containing versions that need builds
          -h, --help
                  Print help
        ");
    }
}
