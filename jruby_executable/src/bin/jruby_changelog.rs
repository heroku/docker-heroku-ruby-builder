use clap::Parser;
use indoc::formatdoc;
use jruby_executable::jruby_build_properties;

#[derive(Parser, Debug)]
struct Args {
    #[arg(long)]
    version: String,
}

#[derive(Debug, thiserror::Error)]
enum Error {
    #[error("{0}")]
    LibError(#[from] jruby_executable::Error),
}

fn jruby_changelog(args: &Args) -> Result<(), Error> {
    let Args { version } = args;

    let stdlib_version = jruby_build_properties(version)
        .map_err(Error::LibError)?
        .ruby_stdlib_version()
        .map_err(Error::LibError)?;

    println!("Add a changelog item: https://devcenter.heroku.com/admin/changelog_items/new");
    println!();

    let changelog = formatdoc! {"
        ## JRuby version {version} is now available

        [JRuby v{version}](/articles/ruby-support#ruby-versions) is now available on Heroku. To run
        your app using this version of Ruby, add the following `ruby` directive to your Gemfile:

        ```ruby
        ruby \"{stdlib_version}\", engine: \"jruby\", engine_version: \"{version}\"
        ```

        The JRuby release notes can be found on the [JRuby website](https://www.jruby.org/news).
    "};

    println!("{changelog}");

    Ok(())
}

fn main() {
    let args = Args::parse();
    if let Err(error) = jruby_changelog(&args) {
        eprintln!("❌ {error}");
        std::process::exit(1);
    }
}
