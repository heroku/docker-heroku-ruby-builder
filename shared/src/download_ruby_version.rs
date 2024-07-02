use crate::Error;
use nom::bytes::complete::tag;
use nom::character::complete::digit1;
use nom::combinator::map_res;
use std::fmt::Display;
use std::str::FromStr;

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

type VerboseResult<T, U> = nom::IResult<T, U, nom::error::VerboseError<T>>;
fn parse_version(input: &str) -> VerboseResult<&str, RubyDownloadVersion> {
    let mut parse_num = map_res(digit1, |s: &str| s.parse::<u32>());
    let (input, major) = parse_num(input)?;
    let (input, _) = tag(".")(input)?;
    let (input, minor) = parse_num(input)?;
    let (input, _) = tag(".")(input)?;
    let (input, patch) = parse_num(input)?;

    Ok((
        "",
        RubyDownloadVersion {
            major,
            minor,
            patch,
            rest: input.to_string(),
        },
    ))
}

impl RubyDownloadVersion {
    pub fn new(s: impl AsRef<str>) -> Result<Self, Error> {
        let (_, version) = parse_version(s.as_ref()).map_err(|err| Error::InvalidVersion {
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
        format!("ruby-{}", self)
    }

    pub fn download_url(&self) -> String {
        format!(
            "https://cache.ruby-lang.org/pub/ruby/{major}.{minor}/ruby-{self}.tar.gz",
            major = self.major,
            minor = self.minor,
        )
    }
}
