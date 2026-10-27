use bullet_stream::global::print;
use clap::Parser;
use fs_err::{self as fs};
use indoc::formatdoc;
use shared::inventory_check;
use std::path::{Path, PathBuf};

#[derive(Parser, Debug)]
struct Args {
    path: PathBuf,
}

async fn check(path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let contents = fs::read_to_string(path)?;
    inventory_check(&contents).await?;
    Ok(())
}

#[tokio::main]
async fn main() {
    let args = Args::parse();
    if let Err(error) = check(&args.path).await {
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
        Usage: inventory_check <PATH>

        Arguments:
          <PATH>

        Options:
          -h, --help  Print help
        ");
    }
}
