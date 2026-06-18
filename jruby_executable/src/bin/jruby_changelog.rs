use std::{error::Error, io::Write};

use bullet_stream::global::print;
use clap::Parser;
use indoc::formatdoc;
use jruby_executable::{JRubyVersion, jruby_build_properties};

#[derive(Parser, Debug)]
struct Args {
    #[arg(long)]
    version: JRubyVersion,
}

fn jruby_changelog<W>(args: &Args, mut io: W) -> Result<W, Box<dyn Error>>
where
    W: Write,
{
    let Args { version } = args;

    let stdlib_version = jruby_build_properties(version)?.ruby_stdlib_version()?;

    let changelog = formatdoc! {"
        ## JRuby version {version} is now available

        [JRuby v{version}](/articles/ruby-support-reference#supported-jruby-versions) is now available on Heroku. To run
        your app using this version of Ruby, add the following `ruby` directive to your Gemfile:

        ```ruby
        ruby \"{stdlib_version}\", engine: \"jruby\", engine_version: \"{version}\"
        ```

        The JRuby release notes can be found on the [JRuby website](https://www.jruby.org/news).
    "};

    writeln!(io, "{changelog}")?;

    Ok(io)
}

fn main() {
    let args = Args::parse();
    if let Err(error) = jruby_changelog(&args, std::io::stdout()) {
        print::error(formatdoc! {"
            ❌ Command failed ❌

            {error}
        "});

        std::process::exit(1);
    }
}
