// See `bin/*.rs` for scripts

pub mod jruby_version;
pub use jruby_version::JRubyVersion;

use reqwest::Url;
use shared::github::{self, GitHubToken};

/// Short: This struct parses a file based on the input jruby version to determine
/// what Ruby version it targets.
///
/// If you run this script with `9.4.3.0` it will return 3.1.4
///
/// Long: JRuby targets a specific Ruby stdlib version, for example JRuby 9.4.3.0
/// implements Ruby 3.1.4 stdlib. When people use jruby they specify both the
/// jruby version and the stdlib version, for example:
///
/// ```ruby
/// # Gemfile
/// ruby "3.1.4", engine: "jruby", engine_version: "9.4.3.0"
/// ```
///
/// Example file for <https://raw.githubusercontent.com/jruby/jruby/9.4.7.0/default.build.properties>
///
/// ```ini
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
    body: Vec<u8>,
    url: Url,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Cannot find `ruby.version=` from {url}.\n Body:\n{body}")]
    CannotParseJrubyStdlibVersion { url: Url, body: String },

    #[error("Failed to fetch {0}")]
    FailedRequest(#[from] reqwest::Error),

    #[error("Failed to parse Java properties {0}")]
    InvalidProperties(#[from] java_properties::PropertiesError),
}

impl BuildProperties {
    pub fn ruby_stdlib_version(&self) -> Result<String, Error> {
        java_properties::read(&self.body[..])
            .map_err(Error::InvalidProperties)
            .map(|properties| {
                properties
                    .get("version.ruby")
                    .map(|version| version.to_owned())
            })?
            .ok_or_else(|| Error::CannotParseJrubyStdlibVersion {
                url: self.url.clone(),
                body: std::str::from_utf8(&self.body)
                    .expect("UTF8 encoded java properties file from")
                    .to_owned(),
            })
    }
}

pub async fn jruby_build_properties(
    jruby_version: &JRubyVersion,
    gh_token: &GitHubToken,
) -> Result<BuildProperties, Error> {
    let url = build_properties_url(jruby_version);
    let response = github::get_raw_with_retry(&url, gh_token).await?;

    Ok(BuildProperties {
        body: response.body.into_bytes(),
        url,
    })
}

fn build_properties_url(jruby_version: &JRubyVersion) -> Url {
    let mut url =
        Url::parse("https://api.github.com/repos/jruby/jruby/contents/default.build.properties")
            .expect("valid build properties URL constant");
    url.query_pairs_mut()
        .append_pair("ref", &jruby_version.to_string());
    url
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
            body: body.as_bytes().to_vec(),
            url: Url::parse("https://example.com").unwrap(),
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
            body: body.as_bytes().to_vec(),
            url: Url::parse("https://example.com").unwrap(),
        };

        assert_eq!(properties.ruby_stdlib_version().unwrap(), "3.1.4");
    }

    #[test]
    fn test_build_properties_url_targets_the_release_tag() {
        let version = JRubyVersion::parse("9.4.7.0").unwrap();

        assert_eq!(
            build_properties_url(&version).as_str(),
            "https://api.github.com/repos/jruby/jruby/contents/default.build.properties?ref=9.4.7.0"
        );
    }
}
