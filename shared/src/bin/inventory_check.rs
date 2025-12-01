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

fn check(path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let contents = fs::read_to_string(path)?;
    inventory_check(&contents)?;
    Ok(())
}

fn main() {
    let args = Args::parse();
    if let Err(error) = check(&args.path) {
        print::error(formatdoc! {"
            ❌ Command failed ❌

            {error}
        "});
        std::process::exit(1);
    }
}
