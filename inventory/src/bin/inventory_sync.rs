use bullet_stream::global::print;
use clap::Parser;
use fs_err as fs;
use shared::NonEmpty;
use shared::inventory::RubyInventory;
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
          -h, --help
                  Print help
        ");
    }
}
