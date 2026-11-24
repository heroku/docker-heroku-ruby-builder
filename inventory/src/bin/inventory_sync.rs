use bullet_stream::global::print;
use clap::Parser;
use fs_err as fs;
use shared::github::{self, GitHubToken, GitHubTokenArg};
use shared::inventory::{RubyInventory, download_sha7_artifact, version_target};
use shared::ruby_lang::{RELEASES_URL, ruby_lang_versions};
use shared::{BuildTarget, JRubyVersion, NonEmpty, RubyDownloadVersion, build_matrix};
use std::collections::BTreeSet;
use std::error::Error;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::Semaphore;
use tokio::task::JoinSet;

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
    if let Err(e) = jruby_call(&args.jruby_inventory).await {
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

        let mut desired: BTreeSet<(RubyDownloadVersion, BuildTarget)> = BTreeSet::new();
        let targets = build_matrix();
        for version in releases.into_iter().filter(|version| version >= min_ruby) {
            for target in &targets {
                desired.insert((version.clone(), target.clone()));
            }
        }
        desired
    };
    print::bullet(format!("Found {} total releases", desired.len()));
    let missing = desired.difference(&held);
    print::bullet(format!(
        "Inventory missing {} total releases",
        missing.clone().count()
    ));

    let added = {
        // Download isn't CPU bound but checksum generation is
        let limit = std::thread::available_parallelism().map_or(4, NonZeroUsize::get);
        let permits = Arc::new(Semaphore::new(limit));

        let mut set = JoinSet::new();
        for (version, target) in missing.cloned() {
            let permits = Arc::clone(&permits);
            set.spawn(async move {
                let _permit = permits.acquire_owned().await?;
                Ok::<_, Box<dyn Error + Send + Sync>>(
                    download_sha7_artifact(&version.tar_url(&target), &(&version).into(), target)
                        .await?,
                )
            });
        }

        let mut added = Vec::new();
        while let Some(joined) = set.join_next().await {
            match joined {
                Ok(Ok(artifact)) => added.push(artifact),
                Ok(Err(error)) => errors.push(error),
                Err(error) => errors.push(error.into()),
            }
        }
        added
    };

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

async fn jruby_call(jruby_inventory: &Path) -> Result<(), NonEmpty<Box<dyn Error>>> {
    let _inventory = read_inventory(jruby_inventory)?;
    Ok(())
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
