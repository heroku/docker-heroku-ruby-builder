use bullet_stream::global::print;
use clap::Parser;
use fs_err as fs;
use indoc::formatdoc;
use shared::{AllowedChanges, inventory_check};
use std::path::PathBuf;

/// ⁉️😄⁉️ - CLI help: what this checks and which artifacts it downloads
#[derive(Parser, Debug)]
struct Args {
    /// ⁉️😄⁉️ - CLI help for `--old`
    #[arg(long)]
    old: PathBuf,

    /// ⁉️😄⁉️ - CLI help for `--new`
    #[arg(long)]
    new: PathBuf,

    /// ⁉️😄⁉️ - CLI help for `--allow-inventory-change`
    #[arg(long, env = "ALLOW_INVENTORY_CHANGE")]
    allow_inventory_change: bool,
}

async fn call(args: &Args) -> Result<(), Box<dyn std::error::Error>> {
    let allowed = if args.allow_inventory_change {
        AllowedChanges::Any
    } else {
        AllowedChanges::AddOnly
    };
    match inventory_check(
        &fs::read_to_string(&args.old)?,
        &fs::read_to_string(&args.new)?,
        allowed,
    )
    .await?
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
    if let Err(error) = call(&args).await {
        print::error(formatdoc! {"
            ❌ Command failed ❌

            {error}
        "});
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
        ⁉️😄⁉️ - CLI help: what this checks and which artifacts it downloads

        Usage: inventory_check [OPTIONS] --old <OLD> --new <NEW>

        Options:
              --old <OLD>               ⁉️😄⁉️ - CLI help for `--old`
              --new <NEW>               ⁉️😄⁉️ - CLI help for `--new`
              --allow-inventory-change  ⁉️😄⁉️ - CLI help for `--allow-inventory-change` [env: ALLOW_INVENTORY_CHANGE=]
          -h, --help                    Print help
        ");
    }
}
