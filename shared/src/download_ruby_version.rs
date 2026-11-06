use crate::Error;
use std::cmp::Ordering;
use std::fmt::Display;
use std::str::FromStr;
use winnow::Parser;
use winnow::ascii::dec_uint;
use winnow::token::literal;

#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(into = "String", try_from = "String")]
pub struct RubyDownloadVersion {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
    pub rest: String,
}

impl RubyDownloadVersion {
    fn sort_key(&self) -> (u32, u32, u32, bool, &str) {
        (
            self.major,
            self.minor,
            self.patch,
            self.rest.is_empty(),
            &self.rest,
        )
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

impl Display for RubyDownloadVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}.{}.{}{}",
            self.major, self.minor, self.patch, self.rest
        )
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

    let rest = input.to_string();
    *input = "";

    Ok(RubyDownloadVersion {
        major,
        minor,
        patch,
        rest,
    })
}

impl RubyDownloadVersion {
    pub fn new(s: impl AsRef<str>) -> Result<Self, Error> {
        let version = parse_version(&mut s.as_ref()).map_err(|err| Error::InvalidVersion {
            version: s.as_ref().to_string(),
            reason: err.to_string(),
        })?;

        Ok(version)
    }

    /// Returns the Some containing the full release version if it is a prerelease version
    pub fn is_prerelease(&self) -> Option<String> {
        if self.rest.is_empty() {
            None
        } else {
            Some(format!("{}.{}.{}", self.major, self.minor, self.patch))
        }
    }

    pub fn bundler_format(&self) -> String {
        format!(
            "{}.{}.{}{}",
            self.major,
            self.minor,
            self.patch,
            self.rest.replacen('-', ".", 1)
        )
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
    fn test_pre_version_serialization() {
        let version = gem_version::GemVersion::from_str(
            &RubyDownloadVersion::new("4.0.0-preview2")
                .unwrap()
                .bundler_format(),
        )
        .unwrap();

        #[derive(Serialize)]
        struct FakeInventory {
            version: gem_version::GemVersion,
        }

        assert_eq!(
            "version = \"4.0.0.preview2\"",
            toml::to_string(&FakeInventory { version }).unwrap().trim()
        );
    }
}
