//! Generate the version tables in the Dev Center article "Heroku Ruby Support"
//!
//! ```term
//! $ cargo run --bin support_article -- --help
//! ```

use bullet_stream::global::print;
use clap::{Args, Parser, Subcommand};
use fs_err as fs;
use indoc::formatdoc;
use jruby_executable::github_releases;
use ruby_executable::ruby_lang::{
    BRANCHES_URL, RELEASES_URL, ruby_lang_branches, ruby_lang_versions,
};
use shared::BaseImage;
use shared::devcenter::article::{Publication, get_article, publish_article};
use shared::devcenter::{DEVCENTER_HOST, token_from_environment};
use shared::github::{self, GitHubTokenArg};
use shared::maybe_err::ResultVec;
use std::error::Error;
use std::fmt::Display;
use std::io::Write;
use std::path::{Path, PathBuf};
use support_article::jruby_table::{self, JRubyTable};
use support_article::ruby_table::{self, RubyTable};
use support_article::update::{Change, replace_table};
use support_article::{ARTICLE_ID, MajorMinor};

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
    /// Print the article with regenerated tables to stdout. Changes nothing on Dev Center.
    Print(PrintArgs),
    /// Write regenerated tables to the live article when they changed.
    Publish(PublishArgs),
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
}

#[derive(Args, Debug)]
#[group(required = true, multiple = false)]
struct Source {
    /// Read the current article Markdown from a file.
    #[arg(long)]
    from_file: Option<PathBuf>,

    /// Read the current article from Dev Center with `HEROKU_DEVCENTER_API_TOKEN`.
    #[arg(long)]
    from_devcenter: bool,
}

#[derive(Args, Debug)]
struct PrintArgs {
    #[command(flatten)]
    tables: TableArgs,

    #[command(flatten)]
    source: Source,
}

#[derive(Args, Debug)]
struct PublishArgs {
    #[command(flatten)]
    tables: TableArgs,

    /// File to append a Markdown summary to, such as `$GITHUB_STEP_SUMMARY`.
    #[arg(long = "summary-file")]
    summary_file: Option<PathBuf>,
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
    jruby: JRubyTable,
    args: TableArgs,
}

impl Tables {
    /// Replaces both tables in `content`, returning the new content and the tables that changed.
    fn update(&self, content: &str) -> Result<(String, Vec<Change>), BoxError> {
        let ruby = replace_table(content, &self.ruby, self.args.minimum_ruby_major_minor)?;
        let jruby = replace_table(
            &ruby.content,
            &self.jruby,
            self.args.minimum_jruby_major_minor,
        )?;
        Ok((
            jruby.content,
            ruby.change.into_iter().chain(jruby.change).collect(),
        ))
    }
}

/// Builds both tables from upstream release data and S3.
///
/// Requests are retried. One that still fails stops the run before anything is written, and the next run starts over.
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

    print::bullet(format!("Fetching {}", *github_releases::RELEASES_URL));
    let jruby_releases = all_ok(
        github_releases::fetch_releases(gh_token).await,
        "JRuby releases",
    )?;

    print::bullet("Checking S3 for JRuby builds");
    let jruby_builds =
        jruby_table::builds(&jruby_releases, args.minimum_jruby_major_minor, gh_token).await?;
    let jruby = JRubyTable::new(
        &jruby_builds,
        args.minimum_jruby_major_minor,
        BaseImage::all(),
    );
    for row in &jruby.rows {
        print::sub_bullet(format!("{} (Ruby {})", row.version, row.ruby_compat));
    }

    Ok(Tables { ruby, jruby, args })
}

fn report_changes(changes: &[Change]) {
    if changes.is_empty() {
        print::bullet("Tables are already current");
    }
    for change in changes {
        print::bullet(format!("Changed {}", change.name));
    }
}

async fn print_article(args: PrintArgs) -> Result<(), BoxError> {
    let tables = generate(args.tables).await?;
    let content = match &args.source.from_file {
        Some(path) => fs::read_to_string(path)?,
        None => {
            let token = token_from_environment().map_err(|error| error.to_string())?;
            get_article(&DEVCENTER_HOST, &token, ARTICLE_ID)
                .await?
                .content
        }
    };

    let (content, changes) = tables.update(&content)?;
    report_changes(&changes);
    std::io::stdout().write_all(content.as_bytes())?;
    Ok(())
}

async fn publish(args: PublishArgs) -> Result<(), BoxError> {
    let token = token_from_environment().map_err(|error| error.to_string())?;
    let tables = generate(args.tables).await?;

    print::bullet(format!(
        "Publishing Dev Center article {ARTICLE_ID} if its tables changed"
    ));
    let mut changes = Vec::new();
    let publication = publish_article(&DEVCENTER_HOST, &token, ARTICLE_ID, |content| {
        let (updated, table_changes) = tables.update(content)?;
        changes = table_changes;
        Ok::<_, BoxError>((!changes.is_empty()).then_some(updated))
    })
    .await?;
    report_changes(&changes);

    let markdown = summary(&publication, &changes);
    print::plain(&markdown);
    if let Some(path) = &args.summary_file
        && let Err(error) = append(path, &markdown)
    {
        print::warning(format!("Failed to write summary: {error}"));
    }
    Ok(())
}

fn append(path: &Path, text: &str) -> std::io::Result<()> {
    fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(path)
        .and_then(|mut file| file.write_all(text.as_bytes()))
}

fn summary(publication: &Publication, changes: &[Change]) -> String {
    let headline = match publication {
        Publication::Unchanged { versions_count } => {
            format!("✅ Article {ARTICLE_ID} is already current at revision {versions_count}")
        }
        Publication::Published {
            url,
            versions_count,
        } => format!("📣 Published revision {versions_count} of {url}"),
    };
    let mut markdown = format!("### Heroku Ruby Support article\n\n{headline}\n");
    for Change {
        name,
        before,
        after,
    } in changes
    {
        markdown.push_str(&formatdoc! {"

            #### `{name}` before

            {before}

            #### `{name}` after

            {after}
        "});
    }
    markdown
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let result = match cli.command {
        Command::Print(args) => print_article(args).await,
        Command::Publish(args) => publish(args).await,
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
    fn summary_reports_an_already_current_article() {
        assert_eq!(
            summary(
                &Publication::Unchanged {
                    versions_count: 487
                },
                &[]
            ),
            "### Heroku Ruby Support article\n\n✅ Article 698 is already current at revision 487\n"
        );
    }

    #[test]
    fn summary_shows_each_changed_table_before_and_after() {
        let published = Publication::Published {
            url: "https://devcenter.heroku.com/articles/ruby-support-reference"
                .parse()
                .unwrap(),
            versions_count: 488,
        };
        let changes = [Change {
            name: "ruby_versions_table",
            before: "<table>old</table>".to_string(),
            after: "<table>new</table>".to_string(),
        }];

        assert_eq!(
            summary(&published, &changes),
            indoc::indoc! {"
                ### Heroku Ruby Support article

                📣 Published revision 488 of https://devcenter.heroku.com/articles/ruby-support-reference

                #### `ruby_versions_table` before

                <table>old</table>

                #### `ruby_versions_table` after

                <table>new</table>
            "}
        );
    }

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
    fn cli_requires_exactly_one_source() {
        let base = [
            "support_article",
            "print",
            "--gh-token=t",
            "--minimum-ruby-major-minor=3.1",
            "--minimum-jruby-major-minor=9.4",
        ];
        assert!(Cli::try_parse_from(base).is_err());
        assert!(Cli::try_parse_from(base.into_iter().chain(["--from-devcenter"])).is_ok());
        assert!(
            Cli::try_parse_from(
                base.into_iter()
                    .chain(["--from-devcenter", "--from-file=a.md"])
            )
            .is_err()
        );
    }

    #[test]
    fn get_help() {
        insta::assert_snapshot!(shared::help(Cli::command(), &[]), @r"
        Generate the Ruby and JRuby version tables in the Heroku Ruby Support Dev Center article

        Usage: support_article <COMMAND>

        Commands:
          print    Print the article with regenerated tables to stdout. Changes nothing on Dev Center
          publish  Write regenerated tables to the live article when they changed
          help     Print this message or the help of the given subcommand(s)

        Options:
          -h, --help  Print help
        ");
    }

    #[test]
    fn get_print_help() {
        insta::assert_snapshot!(shared::help(Cli::command(), &["print"]), @r"
        Print the article with regenerated tables to stdout. Changes nothing on Dev Center

        Usage: support_article print --gh-token <TOKEN> --minimum-ruby-major-minor <MINIMUM_RUBY_MAJOR_MINOR> --minimum-jruby-major-minor <MINIMUM_JRUBY_MAJOR_MINOR> <--from-file <FROM_FILE>|--from-devcenter>

        Options:
              --gh-token <TOKEN>
                  GitHub API token. e.g. `--gh-token=$(gh auth token)` [env: GITHUB_TOKEN]
              --minimum-ruby-major-minor <MINIMUM_RUBY_MAJOR_MINOR>
                  Oldest Ruby `MAJOR.MINOR` line to list, such as `3.1`
              --minimum-jruby-major-minor <MINIMUM_JRUBY_MAJOR_MINOR>
                  Oldest JRuby `MAJOR.MINOR` line to list, such as `9.4`
              --from-file <FROM_FILE>
                  Read the current article Markdown from a file
              --from-devcenter
                  Read the current article from Dev Center with `HEROKU_DEVCENTER_API_TOKEN`
          -h, --help
                  Print help
        ");
    }

    #[test]
    fn get_publish_help() {
        insta::assert_snapshot!(shared::help(Cli::command(), &["publish"]), @r"
        Write regenerated tables to the live article when they changed

        Usage: support_article publish [OPTIONS] --gh-token <TOKEN> --minimum-ruby-major-minor <MINIMUM_RUBY_MAJOR_MINOR> --minimum-jruby-major-minor <MINIMUM_JRUBY_MAJOR_MINOR>

        Options:
              --gh-token <TOKEN>
                  GitHub API token. e.g. `--gh-token=$(gh auth token)` [env: GITHUB_TOKEN]
              --minimum-ruby-major-minor <MINIMUM_RUBY_MAJOR_MINOR>
                  Oldest Ruby `MAJOR.MINOR` line to list, such as `3.1`
              --minimum-jruby-major-minor <MINIMUM_JRUBY_MAJOR_MINOR>
                  Oldest JRuby `MAJOR.MINOR` line to list, such as `9.4`
              --summary-file <SUMMARY_FILE>
                  File to append a Markdown summary to, such as `$GITHUB_STEP_SUMMARY`
          -h, --help
                  Print help
        ");
    }
}
