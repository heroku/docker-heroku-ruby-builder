//! Generate the version tables in the Dev Center article "Heroku Ruby Support"
//!
//! ```term
//! $ cargo run --bin support_article -- --help
//! ```

use bullet_stream::global::print;
use clap::{Args, Parser, Subcommand};
use indoc::formatdoc;
use ruby_executable::ruby_lang::{
    BRANCHES_URL, RELEASES_URL, ruby_lang_branches, ruby_lang_versions,
};
use shared::BaseImage;
use shared::github::{self, GitHubTokenArg};
use shared::maybe_err::ResultVec;
use std::error::Error;
use std::fmt::Display;
use std::io::Write;
use support_article::MajorMinor;
use support_article::ruby_table::{self, RubyTable};

type BoxError = Box<dyn Error + Send + Sync>;

#[derive(Parser, Debug)]
#[command(
    bin_name = "support_article",
    about = "Generate the Ruby version table in the Heroku Ruby Support Dev Center article"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Print the generated table HTML to stdout.
    Print(PrintArgs),
}

#[derive(Args, Debug)]
struct TableArgs {
    #[command(flatten)]
    github: GitHubTokenArg,

    /// Oldest Ruby `MAJOR.MINOR` line to list, such as `3.1`.
    #[arg(long)]
    minimum_ruby_major_minor: MajorMinor,
}

#[derive(Args, Debug)]
struct PrintArgs {
    #[command(flatten)]
    tables: TableArgs,
}

/// Every value, or one error listing every failure.
fn all_ok<T, E: Display>(results: ResultVec<T, E>, what: &str) -> Result<Vec<T>, String> {
    let mut errors: Vec<E> = Vec::new();
    let values = results.unwrap_drain_errs(&mut errors);
    if errors.is_empty() {
        Ok(values)
    } else {
        Err(format!(
            "Failed reading {what}:\n{}",
            errors
                .iter()
                .map(|error| format!("- {error}"))
                .collect::<Vec<_>>()
                .join("\n")
        ))
    }
}

struct Tables {
    ruby: RubyTable,
}

/// Builds the tables from upstream release data and S3.
///
/// Requests are retried. One that still fails stops the run.
async fn generate(args: TableArgs) -> Result<Tables, BoxError> {
    let gh_token = &args.github.token;
    print::bullet(format!("Fetching {}", *RELEASES_URL));
    let releases = all_ok(
        ruby_lang_versions(
            github::get_raw_with_retry(&RELEASES_URL, gh_token)
                .await?
                .body,
        ),
        "Ruby releases",
    )?;
    print::bullet(format!("Fetching {}", *BRANCHES_URL));
    let branches = ruby_lang_branches(
        github::get_raw_with_retry(&BRANCHES_URL, gh_token)
            .await?
            .body,
    )?;

    print::bullet("Checking S3 for Ruby builds");
    let ruby_builds = ruby_table::builds(&releases, args.minimum_ruby_major_minor).await?;
    let ruby = RubyTable::new(
        &branches,
        &ruby_builds,
        args.minimum_ruby_major_minor,
        BaseImage::all(),
    );
    for row in &ruby.rows {
        print::sub_bullet(format!("{} ({:?})", row.major_minor, row.lifecycle));
    }

    Ok(Tables { ruby })
}

async fn print_tables(args: PrintArgs) -> Result<(), BoxError> {
    let tables = generate(args.tables).await?;
    writeln!(std::io::stdout(), "{}", tables.ruby.render())?;
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
    use pretty_assertions::assert_eq;

    #[test]
    fn all_ok_lists_every_failure() {
        let results: ResultVec<u32, String> =
            vec![Ok(1), Err("bad".to_string()), Err("worse".to_string())]
                .into_iter()
                .collect();
        assert_eq!(
            all_ok(results, "numbers"),
            Err("Failed reading numbers:\n- bad\n- worse".to_string())
        );
    }

    #[test]
    fn get_help() {
        insta::assert_snapshot!(shared::help(Cli::command(), &[]), @r"
        Generate the Ruby version table in the Heroku Ruby Support Dev Center article

        Usage: support_article <COMMAND>

        Commands:
          print  Print the generated table HTML to stdout
          help   Print this message or the help of the given subcommand(s)

        Options:
          -h, --help  Print help
        ");
    }

    #[test]
    fn get_print_help() {
        insta::assert_snapshot!(shared::help(Cli::command(), &["print"]), @r"
        Print the generated table HTML to stdout

        Usage: support_article print --gh-token <TOKEN> --minimum-ruby-major-minor <MINIMUM_RUBY_MAJOR_MINOR>

        Options:
              --gh-token <TOKEN>
                  GitHub API token. e.g. `--gh-token=$(gh auth token)` [env: GITHUB_TOKEN]
              --minimum-ruby-major-minor <MINIMUM_RUBY_MAJOR_MINOR>
                  Oldest Ruby `MAJOR.MINOR` line to list, such as `3.1`
          -h, --help
                  Print help
        ");
    }
}
