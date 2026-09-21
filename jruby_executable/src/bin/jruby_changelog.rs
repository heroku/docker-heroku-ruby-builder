use std::{error::Error, io::Write};

use bullet_stream::global::print;
use clap::{Args, Parser, Subcommand};
use indoc::formatdoc;
use jruby_executable::{JRubyVersion, jruby_build_properties};
use shared::devcenter::{
    CreateOutcome, DEVCENTER_HOST, DUPLICATE_WINDOW_DAYS, DevCenterToken, NewChangelogItem, Status,
    create_changelog_item,
};

#[derive(Parser, Debug)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Print the Dev Center changelog markdown to stdout.
    Print(PrintArgs),
    /// Create the changelog entry via the Dev Center private API.
    Devcenter(DevcenterArgs),
}

#[derive(Args, Debug)]
struct PrintArgs {
    #[arg(long)]
    version: JRubyVersion,
}

#[derive(Args, Debug)]
struct DevcenterArgs {
    #[arg(long)]
    version: JRubyVersion,
    /// Whether to create the entry as an unpublished draft or publish it live.
    #[arg(long, value_enum)]
    status: Status,
}

/// The `title` and `content` of a changelog entry, kept separate so the Dev
/// Center API receives them in distinct fields.
struct ChangelogParts {
    title: String,
    content: String,
}

fn jruby_changelog_parts(version: &JRubyVersion, stdlib_version: &str) -> ChangelogParts {
    let title = format!("JRuby version {version} is now available");

    let content = formatdoc! {"
        [JRuby v{version}](/articles/ruby-support-reference#supported-jruby-versions) is now available on Heroku. To run
        your app using this version of Ruby, add the following `ruby` directive to your Gemfile:

        ```ruby
        ruby \"{stdlib_version}\", engine: \"jruby\", engine_version: \"{version}\"
        ```

        The JRuby release notes can be found on the [JRuby website](https://www.jruby.org/news)."};

    ChangelogParts { title, content }
}

fn render_jruby_changelog<W>(
    version: &JRubyVersion,
    stdlib_version: &str,
    mut io: W,
) -> Result<W, Box<dyn Error>>
where
    W: Write,
{
    let ChangelogParts { title, content } = jruby_changelog_parts(version, stdlib_version);
    writeln!(io, "## {title}\n\n{content}")?;
    Ok(io)
}

async fn render(args: &PrintArgs) -> Result<(), Box<dyn Error>> {
    let stdlib_version = jruby_build_properties(&args.version)
        .await?
        .ruby_stdlib_version()?;
    render_jruby_changelog(&args.version, &stdlib_version, std::io::stdout())?;
    Ok(())
}

async fn create(args: &DevcenterArgs) -> Result<(), Box<dyn Error>> {
    let stdlib_version = jruby_build_properties(&args.version)
        .await?
        .ruby_stdlib_version()?;
    let ChangelogParts { title, content } = jruby_changelog_parts(&args.version, &stdlib_version);
    let item = NewChangelogItem {
        title,
        content,
        status: args.status.clone(),
    };

    let token = DevCenterToken::try_from(
        std::env::var("HEROKU_DEVCENTER_API_TOKEN")
            .map_err(|error| format!("HEROKU_DEVCENTER_API_TOKEN is required {error:?}"))?
            .as_str(),
    )?;

    match create_changelog_item(&DEVCENTER_HOST, &token, &item).await? {
        CreateOutcome::Created(created) => {
            println!(
                "Created changelog item id={id} ({state})",
                id = created.id,
                state = if created.is_published() {
                    "published"
                } else {
                    "draft"
                },
            );
            Ok(())
        }
        CreateOutcome::AlreadyPublished(existing) => {
            let id = existing.id;
            let title = existing.title;
            let published_at = existing
                .published_at
                .map_or_else(|| "unknown".to_string(), |at| at.to_rfc3339());
            Err(formatdoc! {"
                Refusing to publish a duplicate changelog entry.

                A matching entry was already published within the last {DUPLICATE_WINDOW_DAYS} days:
                  id:           {id}
                  title:        {title}
                  published_at: {published_at}
            "}
            .into())
        }
    }
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let result = match &cli.command {
        Command::Print(args) => render(args).await,
        Command::Devcenter(args) => create(args).await,
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
mod test {
    use super::*;
    use pretty_assertions::assert_eq;

    fn parts(version: &str) -> ChangelogParts {
        jruby_changelog_parts(&JRubyVersion::parse(version).unwrap(), "3.1.4")
    }

    #[test]
    fn regular_release() {
        let mut io = Vec::new();

        let output =
            render_jruby_changelog(&JRubyVersion::parse("9.4.7.0").unwrap(), "3.1.4", &mut io)
                .unwrap();
        let actual = String::from_utf8_lossy(output);
        let expected = formatdoc! {"
                ## JRuby version 9.4.7.0 is now available

                [JRuby v9.4.7.0](/articles/ruby-support-reference#supported-jruby-versions) is now available on Heroku. To run
                your app using this version of Ruby, add the following `ruby` directive to your Gemfile:

                ```ruby
                ruby \"3.1.4\", engine: \"jruby\", engine_version: \"9.4.7.0\"
                ```

                The JRuby release notes can be found on the [JRuby website](https://www.jruby.org/news).
            "};
        assert_eq!(expected.trim(), actual.trim());
    }

    #[test]
    fn title_is_plain_text_without_heading() {
        let ChangelogParts { title, .. } = parts("9.4.7.0");
        assert_eq!(title, "JRuby version 9.4.7.0 is now available");
    }

    #[test]
    fn content_does_not_repeat_the_title_heading() {
        let ChangelogParts { title, content } = parts("9.4.7.0");
        assert!(!content.starts_with("##"), "content: {content}");
        assert!(
            !content.contains(&title),
            "content should not repeat the title heading: {content}"
        );
    }

    #[test]
    fn devcenter_requires_status() {
        let result = Cli::try_parse_from(["jruby_changelog", "devcenter", "--version", "9.4.7.0"]);
        assert!(result.is_err(), "expected --status to be required");
    }

    #[test]
    fn devcenter_parses_status_draft() {
        let cli = Cli::try_parse_from([
            "jruby_changelog",
            "devcenter",
            "--version",
            "9.4.7.0",
            "--status=draft",
        ])
        .unwrap();
        let Command::Devcenter(args) = cli.command else {
            panic!("expected the devcenter subcommand");
        };
        assert_eq!(args.status, Status::Draft);
    }

    #[test]
    fn devcenter_parses_status_published() {
        let cli = Cli::try_parse_from([
            "jruby_changelog",
            "devcenter",
            "--version",
            "9.4.7.0",
            "--status",
            "published",
        ])
        .unwrap();
        let Command::Devcenter(args) = cli.command else {
            panic!("expected the devcenter subcommand");
        };
        assert_eq!(args.status, Status::Published);
    }
}
