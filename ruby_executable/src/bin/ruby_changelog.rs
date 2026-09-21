use std::{error::Error, io::Write};

use bullet_stream::global::print;
use clap::{Args, Parser, Subcommand};
use indoc::formatdoc;
use shared::RubyDownloadVersion;
use shared::devcenter::{NewChangelogItem, Status, create_and_report};

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
    version: RubyDownloadVersion,
}

#[derive(Args, Debug)]
struct DevcenterArgs {
    #[arg(long)]
    version: RubyDownloadVersion,
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

fn ruby_changelog_parts(version: &RubyDownloadVersion) -> ChangelogParts {
    let gemfile_format = version.bundler_format();

    let title = format!("Ruby version {version} is now available");

    let mut content = formatdoc! {"
        [Ruby v{version}](/articles/ruby-support#ruby-versions) is now available on Heroku. To run your app using this version of Ruby, add the following `ruby` directive to your Gemfile:

        ```ruby
        ruby \"{gemfile_format}\"
        ```

        For more information on [Ruby {version}, you can view the release announcement](https://www.ruby-lang.org/en/news/)."};

    if let Some(full_version) = version.is_prerelease() {
        content.push_str("\n\n");
        content.push_str(&formatdoc! {"
            > Note
            > This version of Ruby is not suitable for production applications.
            > However, it can be used to test that your application is ready for
            > the official release of Ruby {full_version} and
            > to provide feedback to the Ruby core team."});
    }

    ChangelogParts { title, content }
}

fn render_changelog<W>(version: &RubyDownloadVersion, mut io: W) -> Result<W, Box<dyn Error>>
where
    W: Write,
{
    let ChangelogParts { title, content } = ruby_changelog_parts(version);
    writeln!(io, "## {title}\n\n{content}")?;
    Ok(io)
}

async fn create(args: &DevcenterArgs) -> Result<(), Box<dyn Error>> {
    let ChangelogParts { title, content } = ruby_changelog_parts(&args.version);
    create_and_report(&NewChangelogItem {
        title,
        content,
        status: args.status.clone(),
    })
    .await
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let result = match &cli.command {
        Command::Print(args) => render_changelog(&args.version, std::io::stdout()).map(|_| ()),
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
        ruby_changelog_parts(&RubyDownloadVersion::new(version).unwrap())
    }

    #[test]
    fn regular_release() {
        let mut io = Vec::new();
        let version = RubyDownloadVersion::new("3.3.2").unwrap();

        let output = render_changelog(&version, &mut io).unwrap();
        let actual = String::from_utf8_lossy(output);
        let expected = formatdoc! {"
                ## Ruby version 3.3.2 is now available

                [Ruby v3.3.2](/articles/ruby-support#ruby-versions) is now available on Heroku. To run \
                your app using this version of Ruby, add the following `ruby` directive to your Gemfile:

                ```ruby
                ruby \"3.3.2\"
                ```

                For more information on [Ruby 3.3.2, you can view the release announcement](https://www.ruby-lang.org/en/news/).
            "};
        assert_eq!(expected.trim(), actual.trim());
    }

    #[test]
    fn test_pre_release() {
        let mut io = Vec::new();
        let version = RubyDownloadVersion::new("3.1.0-rc1").unwrap();

        let output = render_changelog(&version, &mut io).unwrap();
        let actual = String::from_utf8_lossy(output);
        let expected = formatdoc! {"
                ## Ruby version 3.1.0-rc1 is now available

                [Ruby v3.1.0-rc1](/articles/ruby-support#ruby-versions) is now available on Heroku. To run \
                your app using this version of Ruby, add the following `ruby` directive to your Gemfile:

                ```ruby
                ruby \"3.1.0.rc1\"
                ```

                For more information on [Ruby 3.1.0-rc1, you can view the release announcement](https://www.ruby-lang.org/en/news/).

                > Note
                > This version of Ruby is not suitable for production applications.
                > However, it can be used to test that your application is ready for
                > the official release of Ruby 3.1.0 and
                > to provide feedback to the Ruby core team.
            "};
        assert_eq!(actual.trim(), expected.trim());
    }

    #[test]
    fn title_is_plain_text_without_heading() {
        let ChangelogParts { title, .. } = parts("3.4.1");
        assert_eq!(title, "Ruby version 3.4.1 is now available");
    }

    #[test]
    fn content_does_not_repeat_the_title_heading() {
        let ChangelogParts { title, content } = parts("3.4.1");
        assert!(!content.starts_with("##"), "content: {content}");
        assert!(
            !content.contains(&title),
            "content should not repeat the title heading: {content}"
        );
    }

    #[test]
    fn prerelease_content_includes_the_warning() {
        let ChangelogParts { content, .. } = parts("3.1.0-rc1");
        assert!(content.contains("> Note"), "content: {content}");
    }

    #[test]
    fn devcenter_requires_status() {
        let result = Cli::try_parse_from(["ruby_changelog", "devcenter", "--version", "3.4.1"]);
        assert!(result.is_err(), "expected --status to be required");
    }

    #[test]
    fn devcenter_parses_status_draft() {
        let cli = Cli::try_parse_from([
            "ruby_changelog",
            "devcenter",
            "--version",
            "3.4.1",
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
            "ruby_changelog",
            "devcenter",
            "--version",
            "3.4.1",
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
