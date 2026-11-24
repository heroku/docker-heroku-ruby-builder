use crate::{BuildTarget, Error, MajorMinor, output_ruby_tar_path, s3_url};
use gem_version::GemVersion;
use reqwest::Url;
use std::cmp::Ordering;
use std::fmt::Display;
use std::path::Path;
use std::str::FromStr;
use winnow::Parser;
use winnow::ascii::dec_uint;
use winnow::combinator::{eof, opt, preceded};
use winnow::token::{literal, one_of, rest};

#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(into = "String", try_from = "String")]
pub struct RubyDownloadVersion {
    major: u32,
    minor: u32,
    patch: u32,
    pre: Option<String>,
}

impl RubyDownloadVersion {
    pub fn major(&self) -> u32 {
        self.major
    }

    pub fn minor(&self) -> u32 {
        self.minor
    }

    pub fn patch(&self) -> u32 {
        self.patch
    }

    fn sort_key(&self) -> (u32, u32, u32, bool, Option<&str>) {
        (
            self.major,
            self.minor,
            self.patch,
            self.pre.is_none(),
            self.pre.as_deref(),
        )
    }

    fn format_with_separator(&self, separator: char) -> String {
        let Self {
            major,
            minor,
            patch,
            pre,
        } = self;
        match pre {
            Some(pre) => format!("{major}.{minor}.{patch}{separator}{pre}"),
            None => format!("{major}.{minor}.{patch}"),
        }
    }
}

/// Orders by `major.minor.patch`. Within a patch, prereleases sort before the release.
impl Ord for RubyDownloadVersion {
    fn cmp(&self, other: &Self) -> Ordering {
        self.sort_key().cmp(&other.sort_key())
    }
}

impl PartialOrd for RubyDownloadVersion {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl From<RubyDownloadVersion> for String {
    fn from(v: RubyDownloadVersion) -> Self {
        v.to_string()
    }
}

impl TryFrom<String> for RubyDownloadVersion {
    type Error = Error;

    fn try_from(s: String) -> Result<Self, Self::Error> {
        s.parse()
    }
}

impl From<&RubyDownloadVersion> for MajorMinor {
    fn from(version: &RubyDownloadVersion) -> Self {
        MajorMinor {
            major: version.major,
            minor: version.minor,
        }
    }
}

impl From<&RubyDownloadVersion> for GemVersion {
    fn from(version: &RubyDownloadVersion) -> Self {
        GemVersion::from_str(&version.bundler_format())
            .expect("RubyDownloadVersion::new validates the bundler format as a GemVersion")
    }
}

impl TryFrom<&GemVersion> for RubyDownloadVersion {
    type Error = Error;

    fn try_from(value: &GemVersion) -> Result<Self, Self::Error> {
        Self::new(value.to_string())
    }
}

impl Display for RubyDownloadVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.format_with_separator('-'))
    }
}

impl FromStr for RubyDownloadVersion {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        RubyDownloadVersion::new(s)
    }
}

fn parse_version(input: &mut &str) -> winnow::Result<RubyDownloadVersion> {
    let major = dec_uint.parse_next(input)?;
    literal(".").parse_next(input)?;
    let minor = dec_uint.parse_next(input)?;
    literal(".").parse_next(input)?;
    let patch = dec_uint.parse_next(input)?;
    let pre = opt(preceded(
        one_of(['-', '.']),
        rest.verify(|pre: &str| pre.starts_with(|c: char| c.is_ascii_alphabetic())),
    ))
    .parse_next(input)?;
    eof.parse_next(input)?;

    Ok(RubyDownloadVersion {
        major,
        minor,
        patch,
        pre: pre.map(str::to_string),
    })
}

impl RubyDownloadVersion {
    pub fn new(s: impl AsRef<str>) -> Result<Self, Error> {
        let version = parse_version(&mut s.as_ref()).map_err(|err| Error::InvalidVersion {
            version: s.as_ref().to_string(),
            reason: err.to_string(),
        })?;
        GemVersion::from_str(&version.bundler_format()).map_err(|err| Error::InvalidVersion {
            version: s.as_ref().to_string(),
            reason: err.to_string(),
        })?;

        Ok(version)
    }

    /// Returns the Some containing the full release version if it is a prerelease version
    pub fn is_prerelease(&self) -> Option<String> {
        self.pre
            .as_ref()
            .map(|_| format!("{}.{}.{}", self.major, self.minor, self.patch))
    }

    pub fn bundler_format(&self) -> String {
        self.format_with_separator('.')
    }

    pub fn dir_name_format(&self) -> String {
        format!("ruby-{self}")
    }

    pub fn download_url(&self) -> String {
        format!(
            "https://cache.ruby-lang.org/pub/ruby/{major}.{minor}/ruby-{self}.tar.gz",
            major = self.major,
            minor = self.minor,
        )
    }

    pub fn tar_url(&self, target: &BuildTarget) -> Url {
        s3_url(&output_ruby_tar_path(
            Path::new(""),
            self,
            &target.base_image,
            Some(&target.arch),
        ))
    }
}

#[cfg(test)]
mod test {
    use serde::Serialize;

    use super::*;

    #[test]
    fn orders_numerically_with_prereleases_before_the_release() {
        let mut versions = ["3.10.0", "3.4.0", "3.4.0-rc1", "3.4.0-preview2", "3.9.1"]
            .map(|v| RubyDownloadVersion::new(v).unwrap());
        versions.sort();
        assert_eq!(
            versions.map(|v| v.to_string()),
            ["3.4.0-preview2", "3.4.0-rc1", "3.4.0", "3.9.1", "3.10.0"]
        );
    }

    #[test]
    fn parses_the_bundler_format_as_the_download_format() {
        let bundler = RubyDownloadVersion::new("4.1.0.preview1").unwrap();
        let download = RubyDownloadVersion::new("4.1.0-preview1").unwrap();
        assert_eq!(bundler, download);
        assert_eq!(bundler.to_string(), "4.1.0-preview1");
        assert_eq!(bundler.bundler_format(), "4.1.0.preview1");
        assert_eq!(
            bundler.download_url(),
            "https://cache.ruby-lang.org/pub/ruby/4.1/ruby-4.1.0-preview1.tar.gz"
        );
    }

    #[test]
    fn rejects_malformed_prereleases() {
        assert!(RubyDownloadVersion::new("3.4.0.1").is_err());
        assert!(RubyDownloadVersion::new("3.4.0-1").is_err());
        assert!(RubyDownloadVersion::new("3.4.0-").is_err());
        assert!(RubyDownloadVersion::new("3.4.0rc1").is_err());
    }

    #[test]
    fn rejects_versions_that_are_not_gem_versions() {
        assert!(RubyDownloadVersion::new("3.4.0 foo").is_err());
        assert!(RubyDownloadVersion::new("3.4.0-.1").is_err());
        assert!(RubyDownloadVersion::new("3.4.0-rc 1").is_err());
        assert!(RubyDownloadVersion::new("3.4.0.rc..1").is_err());
    }

    #[test]
    fn try_from_gem_version() {
        let gem = GemVersion::from_str("4.1.0.preview1").unwrap();
        let version = RubyDownloadVersion::try_from(&gem).unwrap();
        assert_eq!(version, RubyDownloadVersion::new("4.1.0-preview1").unwrap());
        assert_eq!(GemVersion::from(&version), gem);

        let gem = GemVersion::from_str("3.4.0-a.-").unwrap();
        assert!(RubyDownloadVersion::try_from(&gem).is_err());
    }

    #[test]
    fn test_pre_version_serialization() {
        let version = GemVersion::from(&RubyDownloadVersion::new("4.0.0-preview2").unwrap());

        #[derive(Serialize)]
        struct FakeInventory {
            version: GemVersion,
        }

        assert_eq!(
            "version = \"4.0.0.preview2\"",
            toml::to_string(&FakeInventory { version }).unwrap().trim()
        );
    }
}
