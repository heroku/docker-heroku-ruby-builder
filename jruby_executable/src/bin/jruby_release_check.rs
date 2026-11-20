//! Check for JRuby releases that aren't pushed to S3 yet
//!
//! ```term
//! $ cargo run --bin jruby_release_check -- --help
//! ```

use bullet_stream::global::print;
use clap::Parser;
use fs_err as fs;
use gem_version::GemVersion;
use jruby_executable::github_releases::{self, RELEASES_URL, s3_url_to_check};
use jruby_executable::{JRubyVersion, jruby_build_properties};
use shared::github::{GitHubToken, GitHubTokenArg};
use shared::inventory_format::{ArtifactKey, artifact_key, parse_inventory};
use shared::inventory_sync::{SyncReport, Wanted, sync_inventory_file};
use shared::{RubyDownloadVersion, RubyInventory, build_matrix, s3_url};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::str::FromStr;
use std::{error::Error, path::PathBuf};
use tokio::task::JoinSet;

#[derive(Parser, Debug)]
#[command(
    about = "Add JRuby releases on Heroku S3 to the inventory, and list the ones missing from S3"
)]
struct Args {
    #[command(flatten)]
    github: GitHubTokenArg,

    /// Minimum JRuby version to build (e.g. 9.4.7.0). Releases >= this version missing from S3 are written to `--output`.
    #[arg(long = "minimum-version", required = true)]
    minimum_version: JRubyVersion,

    /// Path to write JSON output file containing versions that need builds
    #[arg(long = "output", required = true)]
    output: PathBuf,

    /// Inventory file to add every release found on S3 to
    #[arg(long = "inventory", required = true)]
    inventory: PathBuf,

    /// Oldest version to look for on S3 (e.g. 9.2.17.0). Older releases are not added to the inventory.
    #[arg(long = "inventory-minimum-version", required = true)]
    inventory_minimum_version: JRubyVersion,
}

/// Keep only the releases at or above `minimum`, narrowing the full release
/// listing down to the versions worth checking on S3.
///
/// Ordering uses [`JRubyVersion`]'s field-wise comparison (major, then minor,
/// then patch, then extra), so `9.4.7.0` and anything newer is retained while
/// older versions are dropped. The input slice is left untouched; matching
/// versions are cloned into the returned vector.
fn retain_releases_gte(releases: &[JRubyVersion], minimum: &JRubyVersion) -> Vec<JRubyVersion> {
    releases
        .iter()
        .filter(|version| *version >= minimum)
        .cloned()
        .collect()
}

/// ⁉️😄⁉️ - cost: each lookup is a GitHub API call, and why a partly listed older version is left alone
fn needs_lookup(version: &JRubyVersion, minimum: &JRubyVersion, inventory: &RubyInventory) -> bool {
    let Ok(gem_version) = GemVersion::from_str(&version.to_string()) else {
        return true;
    };
    let listed = inventory
        .artifacts
        .iter()
        .filter_map(|artifact| artifact_key(artifact).ok())
        .filter(|key| key.version == gem_version)
        .map(|key| key.target)
        .collect::<BTreeSet<_>>();
    if listed.is_empty() {
        true
    } else if version < minimum {
        false
    } else {
        build_matrix().iter().any(|target| !listed.contains(target))
    }
}

/// Look up the Ruby standard-library version that the given JRuby `version`
/// implements, returning it paired with the version it belongs to.
async fn resolve_stdlib_version(
    version: JRubyVersion,
    gh_token: GitHubToken,
) -> Result<(JRubyVersion, RubyDownloadVersion), Box<dyn Error + Send + Sync>> {
    let stdlib = jruby_build_properties(&version, &gh_token)
        .await
        .and_then(|props| props.ruby_stdlib_version())?;
    Ok((version, stdlib))
}

/// ⁉️😄⁉️ - why the per-base-image archive is listed before the per-arch copy
fn wanted(
    version: &JRubyVersion,
    ruby_stdlib_version: &RubyDownloadVersion,
) -> Result<Vec<Wanted>, gem_version::VersionError> {
    let gem_version = GemVersion::from_str(&version.to_string())?;
    Ok(build_matrix()
        .into_iter()
        .map(|target| {
            let per_base_image = s3_url(
                &Path::new(target.base_image.name())
                    .join(format!("ruby-{ruby_stdlib_version}-jruby-{version}.tgz")),
            );
            let urls = vec![
                per_base_image,
                s3_url_to_check(version, ruby_stdlib_version, &target),
            ];
            Wanted {
                key: ArtifactKey {
                    version: gem_version.clone(),
                    target,
                },
                urls,
            }
        })
        .collect())
}

/// Attaches human-facing stage context (e.g. "resolving stdlib version") to a
/// type-erased error while preserving its `source()` chain, so the integration
/// layer can keep "which phase failed" without falling back to a bare `String`.
#[derive(Debug, thiserror::Error)]
#[error("{context}: {source}")]
struct StageError {
    context: String,
    #[source]
    source: Box<dyn Error + Send + Sync>,
}

async fn call(args: Args) -> Result<(), Vec<Box<dyn Error>>> {
    print::h2("Checking for new JRuby releases");
    print::bullet(format!("Minimum version: {}", args.minimum_version));

    print::h2(format!("Fetching releases from {}", *RELEASES_URL));
    // Type erasure at the last responsible moment: upstream code stays strongly
    // typed for as long as it can, and only here -- where many unrelated failures
    // are integrated into one report -- do we collapse them to `dyn Error`. This is
    // erasure, not stringification: the boxed error still carries its source chain;
    // a `String` would throw that away. Text is produced only when we print/return.
    let mut errors: Vec<Box<dyn Error>> = Vec::new();
    let mut inventory = fs::read_to_string(&args.inventory)
        .map_err(Box::<dyn Error>::from)
        .and_then(|contents| parse_inventory(&contents).map_err(Box::<dyn Error>::from))
        .map_err(|error| vec![error])?;
    let gh_token = &args.github.token;
    let releases = github_releases::fetch_releases(gh_token)
        .await
        .unwrap_drain_errs(&mut errors);

    print::bullet(format!("Found {} non-prerelease versions", releases.len()));

    let versions_to_check = retain_releases_gte(&releases, &args.inventory_minimum_version)
        .into_iter()
        .filter(|version| needs_lookup(version, &args.minimum_version, &inventory))
        .collect::<Vec<_>>();
    print::bullet(format!(
        "Checking {} versions >= {}",
        versions_to_check.len(),
        args.inventory_minimum_version
    ));

    print::bullet("Ruby stdlib versions");
    let mut stdlib_set = JoinSet::new();
    for version in versions_to_check {
        stdlib_set.spawn(resolve_stdlib_version(version, gh_token.clone()));
    }

    let mut resolved = Vec::new();
    while let Some(result) = stdlib_set.join_next().await {
        match result {
            Ok(Ok((version, stdlib))) => {
                print::sub_bullet(format!("{version} -> Ruby stdlib {stdlib}"));
                resolved.push((version, stdlib));
            }
            Ok(Err(e)) => {
                print::warning(format!("Error resolving stdlib version: {e}"));
                errors.push(Box::new(StageError {
                    context: "resolving stdlib version".to_string(),
                    source: e,
                }));
            }
            Err(join_err) => {
                print::warning(format!(
                    "Task panicked resolving stdlib version: {join_err}"
                ));
                errors.push(Box::new(StageError {
                    context: "task panicked resolving stdlib version".to_string(),
                    source: Box::new(join_err),
                }));
            }
        }
    }

    let mut by_gem_version = BTreeMap::new();
    let mut all_wanted = Vec::new();
    for (version, stdlib) in resolved {
        match wanted(&version, &stdlib) {
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

    let mut versions_to_build = Vec::<JRubyVersion>::new();
    for key in &report.missing {
        let Some(version) = by_gem_version.get(&key.version) else {
            continue;
        };
        if version >= &args.minimum_version {
            print::sub_bullet(format!("Missing {key}"));
            if !versions_to_build.contains(version) {
                versions_to_build.push(version.clone());
            }
        }
    }

    fs::write(
        &args.output,
        &serde_json::to_string_pretty(&versions_to_build).map_err(|e| vec![e.into()])?,
    )
    .map_err(|e| vec![e.into()])?;

    if versions_to_build.is_empty() && errors.is_empty() {
        print::bullet("All checked versions are present on S3");
    } else {
        print::h2("Versions needing builds");
        for version in &versions_to_build {
            print::sub_bullet(format!("{version}"));
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

/// Formats a string into a single bullet point item
///
/// Multi line text correctly lines up with the text above it instead of with the dash.
fn bullet_point(s: impl AsRef<str>) -> String {
    let input = s.as_ref();
    if input.is_empty() {
        String::from("- ")
    } else {
        input
            .split_inclusive('\n')
            .enumerate()
            .map(|(line_index, line)| {
                let prefix = if line_index == 0 { "- " } else { "  " };
                prefix.to_string() + line
            })
            .collect()
    }
}

#[tokio::main]
async fn main() {
    let args = Args::parse();
    match call(args).await {
        Ok(()) => print::bullet("Done"),
        Err(errors) => {
            print::error(format!(
                "Failed! Errors:\n{}",
                errors
                    .iter()
                    .map(|error| bullet_point(error.to_string()))
                    .collect::<Vec<_>>()
                    .join("\n")
            ));
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn bullet_point_formatting() {
        assert_eq!("- hello world", bullet_point("hello world"));
        assert_eq!("- ", bullet_point(""));
        assert_eq!(
            indoc::indoc! {"
                - hello
                  world
            "},
            bullet_point("hello\nworld\n")
        );
    }

    #[test]
    fn test_retain_releases_gte() {
        let releases = vec![
            JRubyVersion::parse("10.1.0.0").unwrap(),
            JRubyVersion::parse("9.4.15.0").unwrap(),
            JRubyVersion::parse("9.4.7.0").unwrap(),
            JRubyVersion::parse("9.4.0.0").unwrap(),
            JRubyVersion::parse("9.3.0.0").unwrap(),
        ];
        let min = JRubyVersion::parse("9.4.7.0").unwrap();
        let filtered = retain_releases_gte(&releases, &min);
        let names: Vec<String> = filtered.iter().map(|v| v.to_string()).collect();
        assert_eq!(names, vec!["10.1.0.0", "9.4.15.0", "9.4.7.0"]);
    }

    #[test]
    fn get_help() {
        insta::assert_snapshot!(shared::help(Args::command(), &[]), @r"
        Add JRuby releases on Heroku S3 to the inventory, and list the ones missing from S3

        Usage: jruby_release_check --gh-token <TOKEN> --minimum-version <MINIMUM_VERSION> --output <OUTPUT> --inventory <INVENTORY> --inventory-minimum-version <INVENTORY_MINIMUM_VERSION>

        Options:
              --gh-token <TOKEN>
                  GitHub API token. e.g. `--gh-token=$(gh auth token)` [env: GITHUB_TOKEN]
              --minimum-version <MINIMUM_VERSION>
                  Minimum JRuby version to build (e.g. 9.4.7.0). Releases >= this version missing from S3 are written to `--output`
              --output <OUTPUT>
                  Path to write JSON output file containing versions that need builds
              --inventory <INVENTORY>
                  Inventory file to add every release found on S3 to
              --inventory-minimum-version <INVENTORY_MINIMUM_VERSION>
                  Oldest version to look for on S3 (e.g. 9.2.17.0). Older releases are not added to the inventory
          -h, --help
                  Print help
        ");
    }
}
