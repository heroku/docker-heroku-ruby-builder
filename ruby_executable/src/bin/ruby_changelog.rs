use std::{error::Error, io::Write};

use bullet_stream::Print;
use clap::Parser;
use indoc::formatdoc;
use shared::RubyDownloadVersion;

#[derive(Parser, Debug)]
struct Args {
    #[arg(long)]
    version: RubyDownloadVersion,
}

fn ruby_changelog<W>(args: &Args, mut io: W) -> Result<W, Box<dyn Error>>
where
    W: Write,
{
    let Args { version } = args;

    writeln!(
        io,
        "Add a changelog item: https://devcenter.heroku.com/admin/changelog_items/new"
    )?;

    writeln!(io)?;

    let gemfile_format = version.bundler_format();

    let changelog = formatdoc! {"
        ## Ruby version {version} is now available

        [Ruby v{version}](/articles/ruby-support#ruby-versions) is now available on Heroku. To run \
        your app using this version of Ruby, add the following `ruby` directive to your Gemfile:

        ```ruby
        ruby \"{gemfile_format}\"
        ```

        For more information on [Ruby {version}, you can view the release announcement](https://www.ruby-lang.org/en/news/).
    "};

    writeln!(io, "{changelog}")?;

    if let Some(full_version) = version.is_prerelease() {
        let warning = formatdoc! {"
            > Note
            > This version of Ruby is not suitable for production applications.
            > However, it can be used to test that your application is ready for
            > the official release of Ruby {full_version} and
            > to provide feedback to the Ruby core team.
        "};
        writeln!(io, "{warning}")?;
    }

    Ok(io)
}

fn main() {
    let args = Args::parse();
    if let Err(error) = ruby_changelog(&args, std::io::stdout()) {
        Print::new(std::io::stderr())
            .without_header()
            .error(formatdoc! {"
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

    #[test]
    fn regular_release() {
        let mut io = Vec::new();
        let version = RubyDownloadVersion::new("3.3.2").unwrap();
        let args = Args { version };

        let output = ruby_changelog(&args, &mut io).unwrap();
        let actual = String::from_utf8_lossy(output);
        let expected = formatdoc! {"
                Add a changelog item: https://devcenter.heroku.com/admin/changelog_items/new

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
        let args = Args { version };

        let output = ruby_changelog(&args, &mut io).unwrap();
        let actual = String::from_utf8_lossy(output);
        let expected = formatdoc! {"
                Add a changelog item: https://devcenter.heroku.com/admin/changelog_items/new

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
}
