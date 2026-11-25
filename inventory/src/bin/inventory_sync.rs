use bullet_stream::global::print;
use clap::Parser;
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

async fn call(args: &Args) -> Result<(), NonEmpty<Box<dyn Error>>> {
    print::h1("Syncing inventory");
    let mut errors = Vec::new();

    let banner = format!("Ruby inventory `{}`", args.ruby_inventory.display());
    print::h2(&banner);
    if let Err(e) = ruby_call(
        &args.ruby_inventory,
        &args.min_ruby_version,
        &args.github.token,
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

async fn ruby_call(
    ruby_inventory: &Path,
    min_ruby: &RubyDownloadVersion,
    gh_token: &GitHubToken,
) -> Result<(), NonEmpty<Box<dyn Error>>> {
    let mut errors: Vec<Box<dyn Error>> = Vec::new();
    let mut inventory = read_inventory(ruby_inventory)?;
    let held: BTreeSet<(RubyDownloadVersion, BuildTarget)> =
        version_target(&inventory).map_err(|errors| errors.map(Box::<dyn Error>::from))?;

    let desired = {
        print::bullet(format!("Fetching releases from {}", *RELEASES_URL));
        let releases = match github::get_raw_with_retry(&RELEASES_URL, gh_token).await {
            Ok(response) => ruby_lang_versions(response.body).unwrap_drain_errs(&mut errors),
            Err(e) => {
                errors.push(e.into());
                Vec::new()
            }
        };

        print::bullet(format!("Found {} total releases", releases.len()));
        let mut desired: BTreeSet<(RubyDownloadVersion, BuildTarget)> = BTreeSet::new();
        let targets = build_matrix();
        for version in releases.into_iter().filter(|version| version >= min_ruby) {
            for target in &targets {
                desired.insert((version.clone(), target.clone()));
            }
        }
        desired
    };

    let missing = desired.difference(&held);
    print::bullet(format!(
        "Inventory missing {} total builds",
        missing.clone().count()
    ));

    let (added, mut not_built) = {
        // Download isn't CPU bound but checksum generation is
        let limit = std::thread::available_parallelism().map_or(4, NonZeroUsize::get);
        let permits = Arc::new(Semaphore::new(limit));

        let mut set = JoinSet::new();
        for (version, target) in missing.cloned() {
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

async fn jruby_call(
    jruby_inventory: &Path,
    min_jruby: &JRubyVersion,
    gh_token: &GitHubToken,
) -> Result<(), NonEmpty<Box<dyn Error>>> {
    let mut errors: Vec<Box<dyn Error>> = Vec::new();
    let mut inventory = read_inventory(jruby_inventory)?;
    let held: BTreeSet<(JRubyVersion, BuildTarget)> =
        version_target(&inventory).map_err(|errors| errors.map(Box::<dyn Error>::from))?;

    let desired = {
        print::bullet("Fetching releases");
        let releases = github_releases::fetch_releases(gh_token)
            .await
            .unwrap_drain_errs(&mut errors);

        print::bullet(format!("Found {} total releases", releases.len()));
        let mut desired: BTreeSet<(JRubyVersion, BuildTarget)> = BTreeSet::new();
        let targets = build_matrix();
        for version in releases.into_iter().filter(|version| version >= min_jruby) {
            for target in &targets {
                desired.insert((version.clone(), target.clone()));
            }
        }
        desired
    };

    let missing = desired.difference(&held);
    print::bullet(format!(
        "Inventory missing {} total builds",
        missing.clone().count()
    ));

    let mut missing_by_version: BTreeMap<JRubyVersion, Vec<BuildTarget>> = BTreeMap::new();
    for (version, target) in missing.cloned() {
        missing_by_version.entry(version).or_default().push(target);
    }

    let (added, mut not_built) = {
        // Download isn't CPU bound but checksum generation is
        let limit = std::thread::available_parallelism().map_or(4, NonZeroUsize::get);
        let permits = Arc::new(Semaphore::new(limit));

        let mut set = JoinSet::new();
        for (version, targets) in missing_by_version {
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

    #[test]
    fn get_help() {
        insta::assert_snapshot!(shared::help(Args::command(), &[]), @r"
        Updates an inventory file to match the artifacts on Heroku S3

        Usage: inventory_sync [OPTIONS] --gh-token <TOKEN>

        Options:
              --ruby-inventory <RUBY_INVENTORY>
                  Path to the ruby inventory [default: ruby_inventory.toml]
              --jruby-inventory <JRUBY_INVENTORY>
                  Path to the jruby inventory [default: jruby_inventory.toml]
              --min-ruby-version <MIN_RUBY_VERSION>
                  Minimum Ruby version to check (e.g. 3.2.0). All releases >= this version will be synced [default: 3.2.1]
              --min-jruby-version <MIN_JRUBY_VERSION>
                  Minimum JRuby version to check (e.g. 9.4.7.0). All releases >= this version will be synced [default: 9.4.7.0]
              --gh-token <TOKEN>
                  GitHub API token. e.g. `--gh-token=$(gh auth token)` [env: GITHUB_TOKEN]
          -h, --help
                  Print help
        ");
    }
}
