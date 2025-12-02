use crate::Error;
use std::fmt::Display;
use std::str::FromStr;
use winnow::Parser;
use winnow::ascii::dec_uint;
use winnow::token::literal;

#[derive(Debug, Clone)]
pub struct RubyDownloadVersion {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
    pub rest: String,
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
