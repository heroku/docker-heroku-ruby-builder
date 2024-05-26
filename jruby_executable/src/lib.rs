/// Short: This script parses a file based on the input jruby version to determine
/// what Ruby version it targets.
///
/// If you run this script with `9.4.3.0` it will return 3.1.4
///
/// Long: JRuby targets a specific Ruby stdlib version, for example JRuby 9.4.3.0
/// implements Ruby 3.1.4 stdlib. When people use jruby they specify both the
/// jruby version and the stdlib version, for example:
///
/// ```ignore
/// # Gemfile
/// ruby "3.1.4", engine: "jruby", engine_version: "9.4.3.0"
/// ```
///
/// Example file for <https://raw.githubusercontent.com/jruby/jruby/9.4.7.0/default.build.properties>
///
/// ```ignore
/// # Defaults. To override, create a file called build.properties in
/// #  the same directory and put your changes in that.
/// #src.dir=src
/// test.dir=test
/// lib.dir=lib
/// build.dir=target
/// spec.dir=spec
/// jruby.gem.home=lib/ruby/gems/shared
/// rubyspec.dir=${spec.dir}/ruby
/// rails.git.repo=git://github.com/rails/rails.git
/// rails.dir=${test.dir}/rails
/// mspec.dir=${spec.dir}/mspec
/// mspec.bin=${mspec.dir}/bin/mspec
/// mspec.tar.file=${build.dir}/mspec.tgz
/// spec.tags.dir=${spec.dir}/tags
/// build.lib.dir=test/target
/// parser.dir=core/src/main/java/org/jruby/parser
/// jflex.bin=jflex
/// jay.bin=jay
///
/// jruby.win32ole.gem=jruby-win32ole
/// installer.gems=${jruby.win32ole.gem}
/// test.classes.dir=${test.dir}/target/test-classes
/// release.dir=release
/// test.results.dir=${build.dir}/test-results
/// jruby.launch.memory=1024M
/// rake.args=
/// install4j.executable=/Applications/install4j9/bin/install4jc
///
/// # Ruby versions
/// version.ruby=3.1.4
/// version.ruby.major=3.1
/// version.ruby.minor=4
/// ```
#[derive(Debug)]
pub struct BuildProperties {
    body: String,
    url: String,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Cannot find `ruby.version=` from {url}.\n Body:\n{body}")]
    CannotParseJrubyStdlibVersion { url: String, body: String },

    #[error("Failed to fetch {0}")]
    FailedRequest(#[from] reqwest::Error),
}

impl BuildProperties {
    pub fn ruby_stdlib_version(&self) -> Result<String, Error> {
        self.body
            .lines()
            .find(|line| line.starts_with("version.ruby="))
            .map(|line| line.replace("version.ruby=", ""))
            .ok_or_else(|| Error::CannotParseJrubyStdlibVersion {
                url: self.url.clone(),
                body: self.body.clone(),
            })
    }
}

pub fn jruby_build_properties(jruby_version: &str) -> Result<BuildProperties, Error> {
    let url = format!(
        "https://raw.githubusercontent.com/jruby/jruby/{jruby_version}/default.build.properties",
    );

    let client = reqwest::blocking::Client::new();
    let response = client.get(&url).send().map_err(Error::FailedRequest)?;

    response
        .error_for_status()
        .map_err(Error::FailedRequest)?
        .text()
        .map_err(Error::FailedRequest)
        .map(|body| BuildProperties { body, url })
}

#[cfg(test)]
mod test {
    use indoc::formatdoc;

    use super::*;

    #[test]
    fn test_jruby_stdlib_version_failure() {
        let body = formatdoc! {"
            # Ruby versions
            version.ruby.major=3.1
            version.ruby.minor=4
        "}
        .to_string();
        let properties = BuildProperties {
            body,
            url: "https://example.com".to_string(),
        };

        assert!(properties.ruby_stdlib_version().is_err());
    }

    #[test]
    fn test_jruby_stdlib_version_success() {
        let body = formatdoc! {"
            # Ruby versions
            version.ruby=3.1.4
            version.ruby.major=3.1
            version.ruby.minor=4
        "}
        .to_string();
        let properties = BuildProperties {
            body,
            url: "https://example.com".to_string(),
        };

        assert_eq!(properties.ruby_stdlib_version().unwrap(), "3.1.4");
    }
}
