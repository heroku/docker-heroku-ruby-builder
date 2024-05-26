use std::io::Write;

use clap::Parser;
use indoc::formatdoc;
use inside_docker::RubyDownloadVersion;

#[derive(Parser, Debug)]
struct Args {
    #[arg(long)]
    version: RubyDownloadVersion,
}

#[derive(Debug, thiserror::Error)]
enum Error {
    #[error("{0}")]
    HerokuError(#[from] inside_docker::Error),

    #[error("Write to IO failed {0}")]
    IoWriteFailed(#[from] std::io::Error),
}

fn ruby_changelog<W>(args: &Args, mut io: W) -> Result<W, Error>
where
    W: Write,
{
    let Args { version } = args;

    writeln!(
        io,
        "Add a changelog item: https://devcenter.heroku.com/admin/changelog_items/new"
    )
    .map_err(Error::IoWriteFailed)?;

    writeln!(io).map_err(Error::IoWriteFailed)?;

    let gemfile_format = version.bundler_format();

    let changelog = formatdoc! {"
        ## Ruby version {version} is now available

        [Ruby v{version}](/articles/ruby-support#ruby-versions) is now available on Heroku. To run
        your app using this version of Ruby, add the following `ruby` directive to your Gemfile:

        ```ruby
        ruby \"{gemfile_format}\"
        ```

        For more information on [Ruby {version}, you can view the release announcement](https://www.ruby-lang.org/en/news/).
    "};

    writeln!(io, "{changelog}").map_err(Error::IoWriteFailed)?;

    if let Some(full_version) = version.is_prerelease() {
        let warning = formatdoc! {"
            > Note
            > This version of Ruby is not suitable for production applications.
            > However, it can be used to test that your application is ready for
            > the official release of Ruby {full_version} and
            > to provide feedback to the Ruby core team.
        "};
        writeln!(io, "{warning}").map_err(Error::IoWriteFailed)?;
    }

    Ok(io)
}

fn main() {
    let args = Args::parse();
    if let Err(error) = ruby_changelog(&args, std::io::stdout()) {
        eprintln!("❌ {error}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod test {
    use super::*;

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

                [Ruby v3.3.2](/articles/ruby-support#ruby-versions) is now available on Heroku. To run
                your app using this version of Ruby, add the following `ruby` directive to your Gemfile:

                ```ruby
                ruby \"3.3.2\"
                ```

                For more information on [Ruby 3.3.2, you can view the release announcement](https://www.ruby-lang.org/en/news/).
            "};
        assert_eq!(actual.trim(), expected.trim());
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

                [Ruby v3.1.0-rc1](/articles/ruby-support#ruby-versions) is now available on Heroku. To run
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
