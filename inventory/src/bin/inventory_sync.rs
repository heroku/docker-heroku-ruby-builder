use bullet_stream::global::print;
use clap::Parser;
use fs_err as fs;
use shared::inventory::RubyInventory;
use shared::{JRubyVersion, NonEmpty, RubyDownloadVersion};
use std::error::Error;
use std::path::{Path, PathBuf};

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
}

async fn call(args: &Args) -> Result<(), NonEmpty<Box<dyn Error>>> {
    let mut errors = Vec::new();
    if let Err(e) = ruby_call(&args.ruby_inventory).await {
        errors.extend(e);
    }
    if let Err(e) = jruby_call(&args.jruby_inventory).await {
        errors.extend(e);
    }

    if let Ok(e) = NonEmpty::try_from(errors) {
        Err(e)
    } else {
        Ok(())
    }
}

async fn ruby_call(ruby_inventory: &Path) -> Result<(), NonEmpty<Box<dyn Error>>> {
    let _inventory = read_inventory(ruby_inventory)?;
    Ok(())
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

        Usage: inventory_sync [OPTIONS]

        Options:
              --ruby-inventory <RUBY_INVENTORY>
                  Path to the ruby inventory [default: ruby_inventory.toml]
              --jruby-inventory <JRUBY_INVENTORY>
                  Path to the jruby inventory [default: jruby_inventory.toml]
              --min-ruby-version <MIN_RUBY_VERSION>
                  Minimum Ruby version to check (e.g. 3.2.0). All releases >= this version will be synced [default: 3.2.1]
              --min-jruby-version <MIN_JRUBY_VERSION>
                  Minimum JRuby version to check (e.g. 9.4.7.0). All releases >= this version will be synced [default: 9.4.7.0]
          -h, --help
                  Print help
        ");
    }
}
