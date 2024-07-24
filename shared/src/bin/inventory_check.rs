use clap::Parser;
use indoc::formatdoc;
use shared::inventory_check;
use std::path::{Path, PathBuf};

#[derive(Parser, Debug)]
struct Args {
    path: PathBuf,
}

fn check(path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let contents = fs_err::read_to_string(path)?;
    inventory_check(&contents)?;
    Ok(())
}

fn main() {
    let args = Args::parse();
    if let Err(error) = check(&args.path) {
        bullet_stream::Print::new(std::io::stderr())
            .without_header()
            .error(formatdoc! {"
                ❌ Command failed ❌

                {error}
            "});
        std::process::exit(1);
    }
}
