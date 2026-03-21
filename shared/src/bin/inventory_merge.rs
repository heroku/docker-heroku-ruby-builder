use bullet_stream::{global::print, style};
use clap::Parser;
use fs_err as fs;
use indoc::formatdoc;
use shared::{atomic_inventory_update, merge_inventories, parse_inventory};
use std::path::PathBuf;

#[derive(Parser, Debug)]
struct Args {
    #[arg(long)]
    target: PathBuf,

    sources: Vec<PathBuf>,
}

fn call(args: &Args) -> Result<(), Box<dyn std::error::Error>> {
    print::h2("Merging inventory files");
    print::bullet(format!("Target: {}", args.target.display()));

    if args.sources.is_empty() {
        Err("No source files provided")?;
    }

    let mut inventories = Vec::new();
    for source_path in &args.sources {
        print::bullet(format!(
            "Source: {}",
            style::value(source_path.display().to_string())
        ));

        let source_contents = fs::read_to_string(source_path)?;
        let source_inventory = parse_inventory(&source_contents)?;
        if source_inventory.artifacts.is_empty() {
            print::sub_bullet("Empty source, skipping");
        } else {
            inventories.push(source_inventory);
        }
    }

    if inventories.is_empty() {
        Err("All inventories are empty".to_string())?;
    }

    let mut added = 0usize;
    atomic_inventory_update(&args.target, |target| {
        let before = target.artifacts.len();
        *target = merge_inventories(target, &inventories)?;
        added = target.artifacts.len() - before;
        Ok(())
    })?;

    print::sub_bullet(format!("Added {added} new entries"));

    print::all_done(&None);
    Ok(())
}

fn main() {
    let args = Args::parse();
    if let Err(error) = call(&args) {
        print::error(formatdoc! {"
            ❌ Command failed ❌

            {error}
        "});
        std::process::exit(1);
    }
}
