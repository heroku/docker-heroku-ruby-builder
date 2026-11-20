use bullet_stream::global::print;
use clap::Parser;
use fs_err as fs;
use shared::{AllowedChanges, NonEmpty, inventory_check};
use std::error::Error;
use std::path::PathBuf;

/// Checks the changed contents of an inventory file exist on Heroku S3 w/ correct checksums
#[derive(Parser, Debug)]
struct Args {
    /// Path to the inventory file to compare against.
    #[arg(long)]
    old: PathBuf,

    /// Path to the new inventory file.
    #[arg(long)]
    new: PathBuf,

    /// By default only additions are allowed. Set this flag to allow deletions or modifications.
    #[arg(long)]
    allow_inventory_change: bool,
}

async fn call(args: &Args) -> Result<(), NonEmpty<Box<dyn Error>>> {
    let allowed = if args.allow_inventory_change {
        AllowedChanges::Any
    } else {
        AllowedChanges::AddOnly
    };
    let old = fs::read_to_string(&args.old).map_err(|error| NonEmpty::new(error.into()))?;
    let new = fs::read_to_string(&args.new).map_err(|error| NonEmpty::new(error.into()))?;
    match inventory_check(&old, &new, allowed)
        .await
        .map_err(|errors| errors.map(|error| Box::new(error) as Box<dyn Error>))?
    {
        Some(changes) => {
            print::bullet(format!("Changes: {}", changes.len()));
            for change in &changes {
                print::sub_bullet(change.to_string());
            }
        }
        None => print::bullet("No changes"),
    }
    Ok(())
}

#[tokio::main]
async fn main() {
    let args = Args::parse();
    if let Err(errors) = call(&args).await {
        print::error("❌ Command failed ❌");
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
        Checks the changed contents of an inventory file exist on Heroku S3 w/ correct checksums

        Usage: inventory_check [OPTIONS] --old <OLD> --new <NEW>

        Options:
              --old <OLD>               Path to the inventory file to compare against
              --new <NEW>               Path to the new inventory file
              --allow-inventory-change  By default only additions are allowed. Set this flag to allow deletions or modifications
          -h, --help                    Print help
        ");
    }
}
