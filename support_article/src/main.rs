//! Generate the version tables in the Dev Center article "Heroku Ruby Support"
//!
//! ```term
//! $ cargo run --bin support_article -- --help
//! ```

use bullet_stream::global::print;
use clap::{Args, Parser, Subcommand};
use fs_err as fs;
use indoc::formatdoc;
use jruby_executable::{github_releases, jruby_build_properties};
use ruby_executable::ruby_lang::{
    BRANCHES_URL, RELEASES_URL, fetch_ruby_lang_body, ruby_lang_branches, ruby_lang_versions,
};
use shared::devcenter::article::{PublishedArticle, get_article, publish_article};
use shared::devcenter::{DEVCENTER_HOST, token_from_environment};
use shared::github::GitHubToken;
use shared::maybe_err::ResultVec;
use shared::s3_url_exists;
use std::error::Error;
use std::fmt::Display;
use std::io::Write;
use std::path::{Path, PathBuf};
use support_article::update::{Change, GeneratedTable, update};
use support_article::{ARTICLE_ID, Series, jruby_table, ruby_table, stacks};

type BoxError = Box<dyn Error + Send + Sync>;

#[derive(Parser, Debug)]
#[command(
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
    /// GitHub API token used to list JRuby releases. Generate one locally with: `--gh-token=$(gh auth token)`
    #[arg(long = "gh-token", value_parser = |s: &str| -> Result<GitHubToken, String> {
        GitHubToken::try_from(s).map_err(|error| format!("{error}. Suggestion: generate one locally with: `--gh-token=$(gh auth token)`"))
    })]
    gh_token: GitHubToken,

    /// Oldest Ruby series to list, such as `3.1`.
    #[arg(long)]
    minimum_ruby_series: Series,

    /// Oldest JRuby minor line to list, such as `9.4`.
    #[arg(long)]
    minimum_jruby_version: Series,
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
    let (values, errors): (Vec<_>, Vec<_>) =
        results.into_inner().into_iter().partition(Result::is_ok);
    if errors.is_empty() {
        Ok(values.into_iter().flatten().collect())
    } else {
        Err(format!(
            "Failed reading {what}:\n{}",
            errors
                .into_iter()
                .filter_map(Result::err)
                .map(|error| format!("- {error}"))
                .collect::<Vec<_>>()
                .join("\n")
        ))
    }
}

/// Builds both tables from upstream release data and S3. Any failure aborts so a partial table is never written.
async fn generate(args: &TableArgs) -> Result<Vec<GeneratedTable>, BoxError> {
    let stacks = stacks();

    print::bullet(format!("Fetching {}", *RELEASES_URL));
    let releases = all_ok(
        ruby_lang_versions(fetch_ruby_lang_body(&RELEASES_URL).await?),
        "Ruby releases",
    )?;
    print::bullet(format!("Fetching {}", *BRANCHES_URL));
    let branches = ruby_lang_branches(fetch_ruby_lang_body(&BRANCHES_URL).await?)?;

    print::bullet("Checking S3 for Ruby builds");
    let ruby_rows = ruby_table::collect(
        &branches,
        &releases,
        args.minimum_ruby_series,
        &stacks,
        s3_url_exists,
    )
    .await?;
    for row in &ruby_rows {
        print::sub_bullet(format!("{} ({:?})", row.series, row.lifecycle));
    }

    print::bullet(format!("Fetching {}", *github_releases::RELEASES_URL));
    let jruby_releases = all_ok(
        github_releases::fetch_releases(&args.gh_token).await,
        "JRuby releases",
    )?;

    print::bullet("Checking S3 for JRuby builds");
    let jruby_rows = jruby_table::collect(
        &jruby_releases,
        args.minimum_jruby_version,
        &stacks,
        |version| async move {
            jruby_build_properties(&version)
                .await
                .and_then(|properties| properties.ruby_stdlib_version())
                .map_err(BoxError::from)
        },
        |url| async { s3_url_exists(url).await.map_err(BoxError::from) },
    )
    .await?;
    for row in &jruby_rows {
        print::sub_bullet(format!("{} (Ruby {})", row.version, row.ruby_compat));
    }

    Ok(vec![
        GeneratedTable {
            name: ruby_table::NAME,
            table: ruby_table::render(&ruby_rows, &stacks),
            floor: args.minimum_ruby_series,
            guard_rows: ruby_table::guard_rows,
        },
        GeneratedTable {
            name: jruby_table::NAME,
            table: jruby_table::render(&jruby_rows, &stacks),
            floor: args.minimum_jruby_version,
            guard_rows: jruby_table::guard_rows,
        },
    ])
}

fn report_changes(changes: &[Change]) {
    if changes.is_empty() {
        print::bullet("Tables are already current");
    }
    for change in changes {
        print::bullet(format!("Changed {}", change.name));
    }
}

async fn print_article(args: &PrintArgs) -> Result<(), BoxError> {
    let tables = generate(&args.tables).await?;
    let content = match &args.source.from_file {
        Some(path) => fs::read_to_string(path)?,
        None => {
            let token = token_from_environment().map_err(|error| error.to_string())?;
            get_article(&DEVCENTER_HOST, &token, ARTICLE_ID)
                .await?
                .content
        }
    };

    let update = update(&content, &tables)?;
    report_changes(&update.changes);
    std::io::stdout().write_all(update.content.as_bytes())?;
    Ok(())
}

enum Outcome {
    AlreadyCurrent { versions_count: u64 },
    Published(PublishedArticle),
}

async fn publish(args: &PublishArgs) -> Result<(), BoxError> {
    let token = token_from_environment().map_err(|error| error.to_string())?;
    let tables = generate(&args.tables).await?;

    print::bullet(format!("Reading Dev Center article {ARTICLE_ID}"));
    let article = get_article(&DEVCENTER_HOST, &token, ARTICLE_ID).await?;
    let update = update(&article.content, &tables)?;
    report_changes(&update.changes);

    let outcome = if update.content == article.content {
        Outcome::AlreadyCurrent {
            versions_count: article.versions_count,
        }
    } else {
        print::bullet("Publishing");
        Outcome::Published(
            publish_article(&DEVCENTER_HOST, &token, &article, &update.content).await?,
        )
    };

    let markdown = summary(&outcome, &update.changes);
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

fn summary(outcome: &Outcome, changes: &[Change]) -> String {
    let headline = match outcome {
        Outcome::AlreadyCurrent { versions_count } => {
            format!("✅ Article {ARTICLE_ID} is already current at revision {versions_count}")
        }
        Outcome::Published(published) => format!(
            "📣 Published revision {} of {}",
            published.versions_count, published.url
        ),
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
    let result = match &cli.command {
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
    use pretty_assertions::assert_eq;

    #[test]
    fn summary_reports_an_already_current_article() {
        assert_eq!(
            summary(
                &Outcome::AlreadyCurrent {
                    versions_count: 487
                },
                &[]
            ),
            "### Heroku Ruby Support article\n\n✅ Article 698 is already current at revision 487\n"
        );
    }

    #[test]
    fn summary_shows_each_changed_table_before_and_after() {
        let published = Outcome::Published(PublishedArticle {
            url: "https://devcenter.heroku.com/articles/ruby-support-reference".to_string(),
            versions_count: 488,
        });
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
            "--minimum-ruby-series=3.1",
            "--minimum-jruby-version=9.4",
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
}
