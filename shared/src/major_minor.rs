use crate::RubyDownloadVersion;
use std::fmt;
use std::str::FromStr;

/// A `MAJOR.MINOR` release line, such as Ruby `3.4` or JRuby `10.0`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MajorMinor {
    pub major: u32,
    pub minor: u32,
}

impl fmt::Display for MajorMinor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.major, self.minor)
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("expected a `MAJOR.MINOR` version like `3.4` but got {0:?}")]
pub struct MajorMinorError(String);

impl FromStr for MajorMinor {
    type Err = MajorMinorError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        s.split_once('.')
            .and_then(|(major, minor)| {
                Some(MajorMinor {
                    major: major.parse().ok()?,
                    minor: minor.parse().ok()?,
                })
            })
            .ok_or_else(|| MajorMinorError(s.to_string()))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn major_minor_parses() {
        assert_eq!(
            "3.10".parse::<MajorMinor>(),
            Ok(MajorMinor {
                major: 3,
                minor: 10
            })
        );
        for invalid in ["3", "3.x", "3.4.1", ""] {
            assert!(invalid.parse::<MajorMinor>().is_err(), "{invalid:?}");
        }
    }

    #[test]
    fn major_minor_orders_numerically() {
        let older: MajorMinor = "3.9".parse().unwrap();
        let newer: MajorMinor = "3.10".parse().unwrap();
        assert!(older < newer);
    }

    #[test]
    fn major_minor_displays() {
        assert_eq!(MajorMinor { major: 3, minor: 4 }.to_string(), "3.4");
    }
}
