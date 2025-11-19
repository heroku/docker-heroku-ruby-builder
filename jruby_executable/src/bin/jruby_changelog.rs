use std::error::Error;

use bullet_stream::global::print;
use clap::Parser;
use indoc::formatdoc;
use jruby_executable::jruby_build_properties;

#[derive(Parser, Debug)]
struct Args {
    #[arg(long)]
    version: String,
}

fn jruby_changelog(args: &Args) -> Result<(), Box<dyn Error>> {
    let Args { version } = args;

    let stdlib_version = jruby_build_properties(version)?.ruby_stdlib_version()?;

    println!("Add a changelog item: https://devcenter.heroku.com/admin/changelog_items/new");
    println!();

    let changelog = formatdoc! {"
        ## JRuby version {version} is now available

        [JRuby v{version}](/articles/ruby-support-reference#ruby-versions) is now available on Heroku. To run
        your app using this version of Ruby, add the following `ruby` directive to your Gemfile:

        ```ruby
        ruby \"{stdlib_version}\", engine: \"jruby\", engine_version: \"{version}\"
        ```

        The JRuby release notes can be found on the [JRuby website](https://www.jruby.org/news).
    "};

    print::plain(changelog);

    Ok(())
}

fn main() {
    let args = Args::parse();
    if let Err(error) = jruby_changelog(&args) {
        print::error(formatdoc! {"
            ❌ Command failed ❌

            {error}
        "});

        std::process::exit(1);
    }
}
