use bullet_stream::global::print;
use clap::builder::BoolValueParser;
use clap::builder::TypedValueParser;
use clap::{ArgAction, Parser};
use fs_err as fs;
use jruby_executable::{github_releases, jruby_build_properties};
use shared::github::{self, GitHubToken, GitHubTokenArg};
use shared::inventory::{RubyArtifact, RubyInventory, download_sha7_artifact, version_target};
use shared::ruby_lang::{RELEASES_URL, ruby_lang_versions};
use shared::{
    BuildTarget, JRubyVersion, NonEmpty, RubyDownloadVersion, build_matrix, output_jruby_tar_path,
    s3_url, s3_url_exists,
};
use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::Semaphore;
use tokio::task::JoinSet;
use url::Url;

/// Updates an inventory file to match the artifacts on Heroku S3
#[derive(Parser, Debug)]
struct Args {
    /// Regenerate all version checksums.
    #[arg(long, action = ArgAction::SetTrue, value_parser = BoolValueParser::new().map(RebuildChecksums))]
    rebuild_checksums: RebuildChecksums,

    /// Path to the ruby inventory.
    #[arg(long, default_value = "ruby_inventory.toml")]
    ruby_inventory: PathBuf,

    /// Path to the jruby inventory.
    #[arg(long, default_value = "jruby_inventory.toml")]
    jruby_inventory: PathBuf,

    /// Minimum Ruby version to check (e.g. 3.2.0). All releases >= this version will be synced.
    #[arg(long = "min-ruby-version", default_value = shared::MIN_RUBY_VERSION)]
    min_ruby_version: RubyDownloadVersion,

    /// Minimum JRuby version to check (e.g. 9.4.7.0). All releases >= this version will be synced.
    #[arg(long = "min-jruby-version", default_value = shared::MIN_JRUBY_VERSION)]
    min_jruby_version: JRubyVersion,

    #[command(flatten)]
    github: GitHubTokenArg,
}

#[derive(Debug, Eq, PartialEq, Clone, Copy)]
struct RebuildChecksums(bool);

async fn call(args: &Args) -> Result<(), NonEmpty<Box<dyn Error>>> {
    print::h1("Syncing inventory");
    let mut errors = Vec::new();

    let banner = format!("Ruby inventory `{}`", args.ruby_inventory.display());
    print::h2(&banner);
    if let Err(e) = ruby_call(
        &args.ruby_inventory,
        &args.min_ruby_version,
        &args.github.token,
        args.rebuild_checksums,
    )
    .await
    {
        errors.push(format!("❌ {banner} errors ❌",).into());
        errors.extend(e);
    }

    let banner = format!("JRuby inventory `{}`", args.jruby_inventory.display());
    print::h2(&banner);
    if let Err(e) = jruby_call(
        &args.jruby_inventory,
        &args.min_jruby_version,
        &args.github.token,
    )
    .await
    {
        errors.push(format!("❌ {banner} errors ❌",).into());
        errors.extend(e);
    }

    if let Ok(e) = NonEmpty::try_from(errors) {
        Err(e)
    } else {
        Ok(())
    }
}

fn ruby_builds_to_sync(
    releases: Vec<RubyDownloadVersion>,
    held: BTreeSet<(RubyDownloadVersion, BuildTarget)>,
    min_ruby: &RubyDownloadVersion,
    rebuild_checksums: RebuildChecksums,
) -> BTreeSet<(RubyDownloadVersion, BuildTarget)> {
    let mut desired = BTreeSet::new();
    let targets = build_matrix();
    for version in releases.into_iter().filter(|version| version >= min_ruby) {
        for target in &targets {
            desired.insert((version.clone(), target.clone()));
        }
    }

    match rebuild_checksums {
        RebuildChecksums(true) => desired.extend(held),
        RebuildChecksums(false) => desired.retain(|build| !held.contains(build)),
    }
    desired
}

async fn ruby_call(
    ruby_inventory: &Path,
    min_ruby: &RubyDownloadVersion,
    gh_token: &GitHubToken,
    rebuild_checksums: RebuildChecksums,
) -> Result<(), NonEmpty<Box<dyn Error>>> {
    let mut errors: Vec<Box<dyn Error>> = Vec::new();
    let mut inventory = read_inventory(ruby_inventory)?;
    let held: BTreeSet<(RubyDownloadVersion, BuildTarget)> =
        version_target(&inventory).map_err(|errors| errors.map(Box::<dyn Error>::from))?;

    print::bullet(format!("Fetching releases from {}", *RELEASES_URL));
    let releases = match github::get_raw_with_retry(&RELEASES_URL, gh_token).await {
        Ok(response) => ruby_lang_versions(response.body).unwrap_drain_errs(&mut errors),
        Err(e) => {
            errors.push(e.into());
            Vec::new()
        }
    };
    print::bullet(format!("Found {} total releases", releases.len()));
    let builds_to_sync = ruby_builds_to_sync(releases, held, min_ruby, rebuild_checksums);

    print::bullet(format!("Syncing {} total builds", builds_to_sync.len()));

    let (added, mut not_built) = {
        // Download isn't CPU bound but checksum generation is
        let limit = std::thread::available_parallelism().map_or(4, NonZeroUsize::get);
        let permits = Arc::new(Semaphore::new(limit));

        let mut set = JoinSet::new();
        for (version, target) in builds_to_sync {
            let permits = Arc::clone(&permits);
            set.spawn(async move {
                let _permit = permits.acquire_owned().await?;
                let url = version.tar_url(&target);
                let download = if s3_url_exists(url.clone()).await? {
                    Download::Added(download_sha7_artifact(&url, &(&version).into(), target).await?)
                } else {
                    Download::NotBuilt(url)
                };
                Ok::<_, Box<dyn Error + Send + Sync>>(download)
            });
        }

        let mut added = Vec::new();
        let mut not_built = Vec::new();
        while let Some(joined) = set.join_next().await {
            match joined {
                Ok(Ok(Download::Added(artifact))) => added.push(artifact),
                Ok(Ok(Download::NotBuilt(url))) => not_built.push(url),
                Ok(Err(error)) => errors.push(error),
                Err(error) => errors.push(error.into()),
            }
        }
        (added, not_built)
    };

    not_built.sort();
    for url in not_built {
        print::sub_bullet(format!("Skipping `{url}`, not on S3 yet"));
    }

    for artifact in added {
        print::sub_bullet(format!("Adding `{}`", artifact.url));
        inventory.insert(artifact);
    }

    print::bullet("Writing inventory to disk");
    if let Err(error) = fs::write(ruby_inventory, inventory.to_string()) {
        errors.push(error.into());
    }

    if let Ok(e) = NonEmpty::try_from(errors) {
        Err(e)
    } else {
        Ok(())
    }
}

enum Download {
    Added(RubyArtifact),
    NotBuilt(Url),
}

fn jruby_builds_to_sync(
    releases: Vec<JRubyVersion>,
    held: BTreeSet<(JRubyVersion, BuildTarget)>,
    min_jruby: &JRubyVersion,
) -> BTreeSet<(JRubyVersion, BuildTarget)> {
    let mut desired = BTreeSet::new();
    let targets = build_matrix();
    for version in releases.into_iter().filter(|version| version >= min_jruby) {
        for target in &targets {
            desired.insert((version.clone(), target.clone()));
        }
    }

    desired.retain(|build| !held.contains(build));
    desired
}

async fn jruby_call(
    jruby_inventory: &Path,
    min_jruby: &JRubyVersion,
    gh_token: &GitHubToken,
) -> Result<(), NonEmpty<Box<dyn Error>>> {
    let mut errors: Vec<Box<dyn Error>> = Vec::new();
    let mut inventory = read_inventory(jruby_inventory)?;
    let held: BTreeSet<(JRubyVersion, BuildTarget)> =
        version_target(&inventory).map_err(|errors| errors.map(Box::<dyn Error>::from))?;

    print::bullet("Fetching releases");
    let releases = github_releases::fetch_releases(gh_token)
        .await
        .unwrap_drain_errs(&mut errors);
    print::bullet(format!("Found {} total releases", releases.len()));
    let builds_to_sync = jruby_builds_to_sync(releases, held, min_jruby);

    print::bullet(format!(
        "Inventory missing {} total builds",
        builds_to_sync.len()
    ));

    let mut builds_by_version: BTreeMap<JRubyVersion, Vec<BuildTarget>> = BTreeMap::new();
    for (version, target) in builds_to_sync {
        builds_by_version.entry(version).or_default().push(target);
    }

    let (added, mut not_built) = {
        // Download isn't CPU bound but checksum generation is
        let limit = std::thread::available_parallelism().map_or(4, NonZeroUsize::get);
        let permits = Arc::new(Semaphore::new(limit));

        let mut set = JoinSet::new();
        for (version, targets) in builds_by_version {
            let stdlib = match jruby_build_properties(&version, gh_token)
                .await
                .and_then(|props| props.ruby_stdlib_version())
            {
                Ok(stdlib) => stdlib,
                Err(error) => {
                    errors.push(error.into());
                    continue;
                }
            };

            for target in targets {
                let permits = Arc::clone(&permits);
                let url = s3_url(&output_jruby_tar_path(
                    Path::new(""),
                    &version,
                    &stdlib,
                    &target.base_image,
                    None,
                ));
                let version = version.clone();
                set.spawn(async move {
                    let _permit = permits.acquire_owned().await?;
                    let download = if s3_url_exists(url.clone()).await? {
                        Download::Added(
                            download_sha7_artifact(&url, &(&version).into(), target).await?,
                        )
                    } else {
                        Download::NotBuilt(url)
                    };
                    Ok::<_, Box<dyn Error + Send + Sync>>(download)
                });
            }
        }

        let mut added = Vec::new();
        let mut not_built = Vec::new();
        while let Some(joined) = set.join_next().await {
            match joined {
                Ok(Ok(Download::Added(artifact))) => added.push(artifact),
                Ok(Ok(Download::NotBuilt(url))) => not_built.push(url),
                Ok(Err(error)) => errors.push(error),
                Err(error) => errors.push(error.into()),
            }
        }
        (added, not_built)
    };

    not_built.sort();
    for url in not_built {
        print::sub_bullet(format!("Skipping `{url}`, not on S3 yet"));
    }

    for artifact in added {
        print::sub_bullet(format!("Adding `{}`", artifact.url));
        inventory.insert(artifact);
    }

    print::bullet("Writing inventory to disk");
    if let Err(error) = fs::write(jruby_inventory, inventory.to_string()) {
        errors.push(error.into());
    }

    if let Ok(e) = NonEmpty::try_from(errors) {
        Err(e)
    } else {
        Ok(())
    }
}

fn read_inventory(path: &Path) -> Result<RubyInventory, NonEmpty<Box<dyn Error>>> {
    fs::read_to_string(path)
        .map_err(Box::<dyn Error>::from)
        .and_then(|contents| {
            contents
                .parse::<RubyInventory>()
                .map_err(Box::<dyn Error>::from)
        })
        .map_err(NonEmpty::new)
}

#[tokio::main]
async fn main() {
    let args = Args::parse();
    if let Err(errors) = call(&args).await {
        print::error("❌ Command Failed ❌");
        for error in &errors {
            print::bullet(error.to_string());
        }
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    fn ruby(s: &str) -> RubyDownloadVersion {
        RubyDownloadVersion::new(s).unwrap()
    }

    fn ruby_builds(versions: &[&str]) -> BTreeSet<(RubyDownloadVersion, BuildTarget)> {
        versions
            .iter()
            .flat_map(|v| build_matrix().into_iter().map(|target| (ruby(v), target)))
            .collect()
    }

    #[test]
    fn builds_to_sync_skips_held_and_old_releases() {
        let releases = vec![ruby("3.1.0"), ruby("3.2.0"), ruby("3.3.0"), ruby("3.4.0")];
        let held = ruby_builds(&["3.3.0"]);

        let builds_to_sync =
            ruby_builds_to_sync(releases, held, &ruby("3.2.0"), RebuildChecksums(false));

        assert_eq!(builds_to_sync, ruby_builds(&["3.2.0", "3.4.0"]));
    }

    #[test]
    fn builds_to_sync_includes_every_held_build_on_rebuild() {
        let releases = vec![ruby("3.3.0"), ruby("3.4.0")];
        let held = ruby_builds(&["3.1.0", "3.3.0"]);

        let builds_to_sync =
            ruby_builds_to_sync(releases, held, &ruby("3.2.0"), RebuildChecksums(true));

        assert_eq!(builds_to_sync, ruby_builds(&["3.1.0", "3.3.0", "3.4.0"]));
    }

    fn jruby(s: &str) -> JRubyVersion {
        JRubyVersion::parse(s).unwrap()
    }

    fn jruby_builds(versions: &[&str]) -> BTreeSet<(JRubyVersion, BuildTarget)> {
        versions
            .iter()
            .flat_map(|v| build_matrix().into_iter().map(|target| (jruby(v), target)))
            .collect()
    }

    #[test]
    fn jruby_builds_to_sync_skips_held_and_old_releases() {
        let releases = vec![
            jruby("9.4.6.0"),
            jruby("9.4.7.0"),
            jruby("9.4.8.0"),
            jruby("10.0.0.0"),
        ];
        let held = jruby_builds(&["9.4.8.0"]);

        let builds_to_sync = jruby_builds_to_sync(releases, held, &jruby("9.4.7.0"));

        assert_eq!(builds_to_sync, jruby_builds(&["9.4.7.0", "10.0.0.0"]));
    }

    #[test]
    fn get_help() {
        insta::assert_snapshot!(shared::help(Args::command(), &[]), @r"
        Updates an inventory file to match the artifacts on Heroku S3

        Usage: inventory_sync [OPTIONS] --gh-token <TOKEN>

        Options:
              --rebuild-checksums
                  Regenerate all version checksums
              --ruby-inventory <RUBY_INVENTORY>
                  Path to the ruby inventory [default: ruby_inventory.toml]
              --jruby-inventory <JRUBY_INVENTORY>
                  Path to the jruby inventory [default: jruby_inventory.toml]
              --min-ruby-version <MIN_RUBY_VERSION>
                  Minimum Ruby version to check (e.g. 3.2.0). All releases >= this version will be synced [default: 3.2.1]
              --min-jruby-version <MIN_JRUBY_VERSION>
                  Minimum JRuby version to check (e.g. 9.4.7.0). All releases >= this version will be synced [default: 9.4.7.0]
              --gh-token <TOKEN>
                  GitHub API token. e.g. `GITHUB_TOKEN=$(gh auth token) <command>` [env: GITHUB_TOKEN]
          -h, --help
                  Print help
        ");
    }
}
