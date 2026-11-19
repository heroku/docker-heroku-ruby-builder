use bullet_stream::global::print;
use clap::Parser;
use fs_err as fs;
use indoc::formatdoc;
use shared::inventory_format::{added_artifacts, check_inventory_format, parse_inventory};
use shared::verify_checksums;
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(
    about = "Check an inventory file is in canonical form, only adds entries to `--base`, and that each added entry downloads with its checksum"
)]
struct Args {
    #[arg(help = "The inventory file to check")]
    path: PathBuf,

    #[arg(
        long,
        help = "The same inventory file on the base branch. Without it, every entry counts as added"
    )]
    base: Option<PathBuf>,
}

async fn check(args: &Args) -> Result<(), Box<dyn std::error::Error>> {
    let inventory = check_inventory_format(&fs::read_to_string(&args.path)?)?;
    let base = match &args.base {
        Some(base) => {
            let contents = fs::read_to_string(base)?;
            if contents.trim().is_empty() {
                Default::default()
            } else {
                parse_inventory(&contents)?
            }
        }
        None => Default::default(),
    };
    let added = added_artifacts(&base, &inventory)?;
    print::bullet(format!(
        "{}: {} artifacts, {} added",
        args.path.display(),
        inventory.artifacts.len(),
        added.len()
    ));
    for artifact in &added {
        print::sub_bullet(&artifact.url);
    }
    verify_checksums(added).await?;
    Ok(())
}

#[tokio::main]
async fn main() {
    let args = Args::parse();
    if let Err(error) = check(&args).await {
        print::error(formatdoc! {"
            ❌ Command failed ❌

            {path}: {error}
        ", path = args.path.display()});
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
        Check an inventory file is in canonical form, only adds entries to `--base`, and that each added entry downloads with its checksum

        Usage: inventory_check [OPTIONS] <PATH>

        Arguments:
          <PATH>  The inventory file to check

        Options:
              --base <BASE>  The same inventory file on the base branch. Without it, every entry counts as added
          -h, --help         Print help
        ");
    }
}
