use bullet_stream::global::print;
use clap::Parser;
use fs_err as fs;
use gem_version::GemVersion;
use reqwest::Url;
use ruby_executable::ruby_lang::{RELEASES_URL, ruby_lang_versions};
use shared::github::{self, GitHubTokenArg};
use shared::inventory_sync::{SyncReport, Wanted, sync_inventory_file};
use shared::{
    ArtifactKey, BuildTarget, NonEmpty, RubyDownloadVersion, RubyInventory, build_matrix,
    output_ruby_tar_path, s3_url,
};
use std::collections::BTreeMap;
use std::str::FromStr;
use std::{
    error::Error,
    path::{Path, PathBuf},
};
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

    /// Path to an inventory file e.g. `ruby_inventory.toml`
    #[arg(long)]
    inventory: PathBuf,
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

fn tar_url(version: &RubyDownloadVersion, target: &BuildTarget) -> Url {
    s3_url(&output_ruby_tar_path(
        Path::new(""),
        version,
        &target.base_image,
        Some(&target.arch),
    ))
}

fn wanted(version: &RubyDownloadVersion) -> Result<Vec<Wanted>, gem_version::VersionError> {
    let gem_version = GemVersion::from_str(&version.bundler_format())?;
    Ok(build_matrix()
        .into_iter()
        .map(|target| Wanted {
            url: tar_url(version, &target),
            key: ArtifactKey {
                version: gem_version.clone(),
                target,
            },
        })
        .collect())
}

async fn call(args: Args) -> Result<(), NonEmpty<Box<dyn Error>>> {
    print::h2("Checking for new Ruby releases");
    print::bullet(format!("Minimum version: {}", args.minimum_version));

    let mut errors: Vec<Box<dyn Error>> = Vec::new();
    let mut inventory = match fs::read_to_string(&args.inventory)
        .map_err(Box::<dyn Error>::from)
        .and_then(|contents| {
            contents
                .parse::<RubyInventory>()
                .map_err(Box::<dyn Error>::from)
        }) {
        Ok(inventory) => inventory,
        Err(error) => return Err(NonEmpty::new(error)),
    };
    print::bullet(format!(
        "Inventory lists {} builds",
        inventory.artifacts().len()
    ));

    print::h2(format!("Fetching releases from {}", *RELEASES_URL));
    let releases = match github::get_raw_with_retry(&RELEASES_URL, &args.github.token).await {
        Ok(response) => ruby_lang_versions(response.body).unwrap_drain_errs(&mut errors),
        Err(e) => {
            errors.push(e.into());
            Vec::new()
        }
    };
    print::bullet(format!("Found {} total releases", releases.len()));

    let mut by_gem_version = BTreeMap::new();
    let mut all_wanted = Vec::new();
    for version in retain_releases_gte(&releases, &args.minimum_version) {
        match wanted(&version) {
            Ok(wanted) => {
                by_gem_version.insert(wanted[0].key.version.clone(), version);
                all_wanted.extend(wanted);
            }
            Err(error) => errors.push(Box::new(error)),
        }
    }

    print::bullet(format!(
        "Looking up {} builds on S3 that the inventory does not list",
        all_wanted.len()
    ));
    let report = match sync_inventory_file(&args.inventory, &mut inventory, all_wanted).await {
        Ok(report) => report,
        Err(error) => {
            errors.push(error.into());
            SyncReport::default()
        }
    };
    for key in &report.added {
        print::sub_bullet(format!("Added {key}"));
    }
    errors.extend(
        report
            .errors
            .into_iter()
            .map(|error| Box::new(error) as Box<dyn Error>),
    );

    let mut versions_to_build = Vec::<RubyDownloadVersion>::new();
    for key in &report.missing {
        let Some(version) = by_gem_version.get(&key.version) else {
            continue;
        };
        print::sub_bullet(format!("Missing {key}"));
        if versions_to_build.last() != Some(version) {
            versions_to_build.push(version.clone());
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
    NonEmpty::try_from(errors).map_or(Ok(()), Err)
}

#[tokio::main]
async fn main() {
    let args = Args::parse();
    match call(args).await {
        Ok(()) => print::bullet("done"),
        Err(errors) => {
            print::error("❌ Failed ❌");
            for error in &errors {
                print::bullet(error.to_string());
            }
            std::process::exit(1);
        }
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

        Usage: ruby_release_check --gh-token <TOKEN> --minimum-version <MINIMUM_VERSION> --output <OUTPUT> --inventory <INVENTORY>

        Options:
              --gh-token <TOKEN>
                  GitHub API token. e.g. `--gh-token=$(gh auth token)` [env: GITHUB_TOKEN]
              --minimum-version <MINIMUM_VERSION>
                  Minimum Ruby version to check (e.g. 3.2.0). All releases >= this version will be checked
              --output <OUTPUT>
                  Path to write JSON output file containing versions that need builds
              --inventory <INVENTORY>
                  Path to an inventory file e.g. `ruby_inventory.toml`
          -h, --help
                  Print help
        ");
    }
}
