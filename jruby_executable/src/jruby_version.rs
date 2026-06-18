//! JRuby's four-segment version scheme: `major.minor.patch.extra` (e.g. `9.4.7.0`).
//!
//! The first three segments roughly track semantic versioning. The fourth
//! (`extra`) is a JRuby-specific maintenance level, bumped for JRuby-internal
//! fixes that don't change the implemented Ruby language version. JRuby uses
//! four segments to keep its own version distinct from the Ruby version it
//! implements (e.g. JRuby `9.4.7.0` implements Ruby `3.1.x`).

use std::fmt;

/// A parsed JRuby version: four numeric segments, `major.minor.patch.extra`.
///
/// Ordering is field-wise (major, then minor, then patch, then extra), so
/// `9.4.7.0 < 9.4.15.0 < 10.1.0.0`. Construct one by parsing a string via
/// [`JRubyVersion::parse`] or [`str::parse`]; the fields are kept private so
/// every value is guaranteed to have come through validation.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct JRubyVersion {
    major: u32,
    minor: u32,
    patch: u32,
    extra: u32,
}

/// Error returned when a string cannot be parsed into a [`JRubyVersion`].
#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    #[error("invalid JRuby version `{input}`: expected 4 parts (major.minor.patch.extra), found {found}")]
    WrongPartCount { input: String, found: usize },
    #[error("invalid JRuby version `{input}`: {component} component `{raw}` is not a number: {source}")]
    InvalidComponent {
        input: String,
        component: &'static str,
        raw: String,
        #[source]
        source: std::num::ParseIntError,
    },
}

impl JRubyVersion {
    /// Parse a version string like `9.4.7.0`. An ergonomic alternative to
    /// `FromStr`; delegates to it, so there is a single parsing implementation.
    pub fn parse(s: &str) -> Result<Self, ParseError> {
        s.parse()
    }
}

impl fmt::Display for JRubyVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}.{}.{}.{}",
            self.major, self.minor, self.patch, self.extra
        )
    }
}

impl serde::Serialize for JRubyVersion {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

impl std::str::FromStr for JRubyVersion {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let parts: Vec<&str> = s.split('.').collect();
        if parts.len() != 4 {
            return Err(ParseError::WrongPartCount {
                input: s.to_owned(),
                found: parts.len(),
            });
        }
        Ok(Self {
            major: parse_component(s, "major", parts[0])?,
            minor: parse_component(s, "minor", parts[1])?,
            patch: parse_component(s, "patch", parts[2])?,
            extra: parse_component(s, "extra", parts[3])?,
        })
    }
}

fn parse_component(input: &str, component: &'static str, raw: &str) -> Result<u32, ParseError> {
    raw.parse().map_err(|source| ParseError::InvalidComponent {
        input: input.to_owned(),
        component,
        raw: raw.to_owned(),
        source,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_valid_version() {
        let v = JRubyVersion::parse("9.4.7.0").unwrap();
        assert_eq!(v.major, 9);
        assert_eq!(v.minor, 4);
        assert_eq!(v.patch, 7);
        assert_eq!(v.extra, 0);
    }

    #[test]
    fn test_parse_invalid_versions() {
        assert!(JRubyVersion::parse("9.4.7").is_err());
        assert!(JRubyVersion::parse("9.4").is_err());
        assert!(JRubyVersion::parse("9").is_err());
        assert!(JRubyVersion::parse("").is_err());
        assert!(JRubyVersion::parse("9.4.7.0.1").is_err());
        assert!(JRubyVersion::parse("a.b.c.d").is_err());
    }

    #[test]
    fn test_parse_errors_show_input_and_source() {
        let wrong_parts = JRubyVersion::parse("9.4.7").unwrap_err().to_string();
        assert!(wrong_parts.contains("9.4.7"), "got: {wrong_parts}");
        assert!(wrong_parts.contains("found 3"), "got: {wrong_parts}");

        let bad_component = JRubyVersion::parse("9.4.x.0").unwrap_err().to_string();
        assert!(bad_component.contains("9.4.x.0"), "got: {bad_component}");
        assert!(bad_component.contains("patch"), "got: {bad_component}");
        assert!(bad_component.contains('x'), "got: {bad_component}");
        // The wrapped ParseIntError message is surfaced, not just chained.
        assert!(
            bad_component.contains("invalid digit"),
            "got: {bad_component}"
        );
    }

    #[test]
    fn test_version_display() {
        let v = JRubyVersion::parse("9.4.7.0").unwrap();
        assert_eq!(v.to_string(), "9.4.7.0");

        let v = JRubyVersion::parse("10.1.0.0").unwrap();
        assert_eq!(v.to_string(), "10.1.0.0");
    }

    #[test]
    fn test_version_ordering() {
        let v940 = JRubyVersion::parse("9.4.0.0").unwrap();
        let v947 = JRubyVersion::parse("9.4.7.0").unwrap();
        let v9415 = JRubyVersion::parse("9.4.15.0").unwrap();
        let v1010 = JRubyVersion::parse("10.1.0.0").unwrap();

        assert!(v947 > v940);
        assert!(v9415 > v947);
        assert!(v1010 > v9415);
        assert!(v940 < v947);
        assert_eq!(v947, JRubyVersion::parse("9.4.7.0").unwrap());
    }

    #[test]
    fn test_version_from_str() {
        let v: JRubyVersion = "9.4.7.0".parse().unwrap();
        assert_eq!(v.to_string(), "9.4.7.0");
    }

    #[test]
    fn test_version_serialize_json() {
        let v = JRubyVersion::parse("9.4.7.0").unwrap();
        let json = serde_json::to_string(&v).unwrap();
        assert_eq!(json, "\"9.4.7.0\"");

        let versions = vec![
            JRubyVersion::parse("10.1.0.0").unwrap(),
            JRubyVersion::parse("9.4.15.0").unwrap(),
        ];
        let json = serde_json::to_string_pretty(&versions).unwrap();
        assert_eq!(json, "[\n  \"10.1.0.0\",\n  \"9.4.15.0\"\n]");
    }
}
