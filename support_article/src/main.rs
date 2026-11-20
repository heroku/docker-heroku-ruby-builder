//! Generate the version tables in the Dev Center article "Heroku Ruby Support"
//!
//! ```term
//! $ cargo run --bin support_article -- --help
//! ```

use bullet_stream::global::print;
use clap::{Args, Parser, Subcommand};
use fs_err as fs;
use indoc::formatdoc;
use ruby_executable::ruby_lang::{BRANCHES_URL, ruby_lang_branches};
use shared::github::{self, GitHubTokenArg};
use shared::{MajorMinor, RubyInventory};
use std::error::Error;
use std::path::{Path, PathBuf};
use support_article::jruby_table::{self, JRubyTable};
use support_article::ruby_table::{self, RubyTable};

type BoxError = Box<dyn Error + Send + Sync>;

#[derive(Parser, Debug)]
#[command(
    bin_name = "support_article",
    about = "Generate the Ruby and JRuby version tables in the Heroku Ruby Support Dev Center article"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Print the generated table HTML.
    Print(PrintArgs),
}

#[derive(Args, Debug)]
struct TableArgs {
    #[command(flatten)]
    github: GitHubTokenArg,

    /// Oldest Ruby `MAJOR.MINOR` line to list, such as `3.1`.
    #[arg(long)]
    minimum_ruby_major_minor: MajorMinor,

    /// Oldest JRuby `MAJOR.MINOR` line to list, such as `9.4`.
    #[arg(long)]
    minimum_jruby_major_minor: MajorMinor,

    /// Inventory file that lists the Ruby builds.
    #[arg(long, default_value = "ruby_inventory.toml")]
    ruby_inventory: PathBuf,

    /// Inventory file that lists the JRuby builds.
    #[arg(long, default_value = "jruby_inventory.toml")]
    jruby_inventory: PathBuf,
}

#[derive(Args, Debug)]
struct PrintArgs {
    #[command(flatten)]
    tables: TableArgs,
}

struct Tables {
    ruby: RubyTable,
    jruby: JRubyTable,
}

fn read_inventory(path: &Path) -> Result<RubyInventory, BoxError> {
    Ok(fs::read_to_string(path)?.parse()?)
}

/// Builds the tables from the inventory files and the upstream Ruby branch status.
async fn generate(args: TableArgs) -> Result<Tables, BoxError> {
    print::bullet(format!("Fetching {}", *BRANCHES_URL));
    let branches = ruby_lang_branches(
        github::get_raw_with_retry(&BRANCHES_URL, &args.github.token)
            .await?
            .body,
    )?;

    print::bullet(format!("Reading {}", args.ruby_inventory.display()));
    let ruby_builds = ruby_table::builds(
        &read_inventory(&args.ruby_inventory)?,
        args.minimum_ruby_major_minor,
    )?;
    let ruby = RubyTable::new(&branches, &ruby_builds, args.minimum_ruby_major_minor)?;
    for row in &ruby.rows {
        print::sub_bullet(format!("{} ({:?})", row.major_minor, row.lifecycle));
    }

    print::bullet(format!("Reading {}", args.jruby_inventory.display()));
    let jruby_builds = jruby_table::builds(
        &read_inventory(&args.jruby_inventory)?,
        args.minimum_jruby_major_minor,
    )?;
    let jruby = JRubyTable::new(&jruby_builds, args.minimum_jruby_major_minor)?;
    for row in &jruby.rows {
        print::sub_bullet(row.major_minor.to_string());
    }

    Ok(Tables { ruby, jruby })
}

async fn print_tables(args: PrintArgs) -> Result<(), BoxError> {
    print::h1("Heroku Ruby Support reference");

    let tables = generate(args.tables).await?;
    let ruby = tables.ruby.render()?;
    let jruby = tables.jruby.render()?;
    print::h2("Ruby Table");
    bullet_stream::global::with_locked_writer(std::io::stdout(), || {
        print::plain(ruby);
    });

    print::h2("JRuby Table");
    bullet_stream::global::with_locked_writer(std::io::stdout(), || {
        print::plain(jruby);
    });
    Ok(())
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let result = match cli.command {
        Command::Print(args) => print_tables(args).await,
    };

    if let Err(error) = result {
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
        insta::assert_snapshot!(shared::help(Cli::command(), &[]), @r"
        Generate the Ruby and JRuby version tables in the Heroku Ruby Support Dev Center article

        Usage: support_article <COMMAND>

        Commands:
          print  Print the generated table HTML
          help   Print this message or the help of the given subcommand(s)

        Options:
          -h, --help  Print help
        ");
    }

    #[test]
    fn get_print_help() {
        insta::assert_snapshot!(shared::help(Cli::command(), &["print"]), @r"
        Print the generated table HTML

        Usage: support_article print [OPTIONS] --gh-token <TOKEN> --minimum-ruby-major-minor <MINIMUM_RUBY_MAJOR_MINOR> --minimum-jruby-major-minor <MINIMUM_JRUBY_MAJOR_MINOR>

        Options:
              --gh-token <TOKEN>
                  GitHub API token. e.g. `--gh-token=$(gh auth token)` [env: GITHUB_TOKEN]
              --minimum-ruby-major-minor <MINIMUM_RUBY_MAJOR_MINOR>
                  Oldest Ruby `MAJOR.MINOR` line to list, such as `3.1`
              --minimum-jruby-major-minor <MINIMUM_JRUBY_MAJOR_MINOR>
                  Oldest JRuby `MAJOR.MINOR` line to list, such as `9.4`
              --ruby-inventory <RUBY_INVENTORY>
                  Inventory file that lists the Ruby builds [default: ruby_inventory.toml]
              --jruby-inventory <JRUBY_INVENTORY>
                  Inventory file that lists the JRuby builds [default: jruby_inventory.toml]
          -h, --help
                  Print help
        ");
    }
}
