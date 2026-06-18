//! JRuby's four-segment version scheme: `major.minor.patch.extra` (e.g. `9.4.7.0`).
//!
//! The first three segments roughly track semantic versioning. The fourth
//! (`extra`) is a JRuby-specific maintenance level, bumped for JRuby-internal
//! fixes that don't change the implemented Ruby language version. JRuby uses
//! four segments to keep its own version distinct from the Ruby version it
//! implements (e.g. JRuby `9.4.7.0` implements Ruby `3.1.x`).

use std::fmt;
use winnow::Parser;
use winnow::ascii::dec_uint;
use winnow::combinator::{eof, seq};
use winnow::error::{StrContext, StrContextValue};
use winnow::token::literal;

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
///
/// Holds the structured winnow [`ContextError`](winnow::error::ContextError)
/// alongside the offending input and the byte offset of the failure, so
/// [`Display`](fmt::Display) can render a caret diagnostic without round-tripping
/// through a pre-formatted string.
#[derive(Debug)]
pub struct ParseError {
    input: String,
    offset: usize,
    inner: winnow::error::ContextError,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "{}", self.input)?;
        write!(f, "{:offset$}^", "", offset = self.offset)?;
        writeln!(f)?;
        write!(f, "{}", self.inner)
    }
}

impl std::error::Error for ParseError {}

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
        parse_version.parse(s).map_err(|e| ParseError {
            input: s.to_owned(),
            offset: e.offset(),
            inner: e.into_inner(),
        })
    }
}

/// A single version segment: a non-negative integer with no sign or whitespace.
fn segment(input: &mut &str) -> winnow::Result<u32> {
    dec_uint
        .context(StrContext::Expected(StrContextValue::Description(
            "non-negative integer",
        )))
        .parse_next(input)
}

/// The literal `.` separator between segments.
fn dot(input: &mut &str) -> winnow::Result<()> {
    literal('.')
        .context(StrContext::Expected(StrContextValue::CharLiteral('.')))
        .void()
        .parse_next(input)
}

fn parse_version(input: &mut &str) -> winnow::Result<JRubyVersion> {
    seq! {JRubyVersion {
        major: segment,
        _: dot,
        minor: segment,
        _: dot,
        patch: segment,
        _: dot,
        extra: segment,
        _: eof.context(StrContext::Expected(StrContextValue::Description("end of input"))),
    }}
    .context(StrContext::Label(
        "JRuby version (`<major>.<minor>.<patch>.<extra>`)",
    ))
    .parse_next(input)
}

#[cfg(test)]
mod tests {
    use super::*;
    use indoc::indoc;

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

        // A leading `+` is accepted by `u32::from_str` but is not a valid
        // JRuby version; winnow's `dec_uint` rejects it.
        assert!(JRubyVersion::parse("+9.4.1.0").is_err());
        // Leading whitespace, empty segments, a trailing dot, and `u32`
        // overflow are all rejected.
        assert!(JRubyVersion::parse(" 0.0.0.0").is_err());
        assert!(JRubyVersion::parse("9..7.0").is_err());
        assert!(JRubyVersion::parse("9.4.7.").is_err());
        assert!(JRubyVersion::parse("99999999999.0.0.0").is_err());
    }

    #[test]
    fn test_parse_error_renders_caret_diagnostic() {
        assert_eq!(
            indoc! {"
                +9.4.1.0
                ^
                invalid JRuby version (`<major>.<minor>.<patch>.<extra>`)
                expected non-negative integer"},
            JRubyVersion::parse("+9.4.1.0").unwrap_err().to_string(),
        );

        assert_eq!(
            indoc! {"
                9..1.0
                  ^
                invalid JRuby version (`<major>.<minor>.<patch>.<extra>`)
                expected non-negative integer"},
            JRubyVersion::parse("9..1.0").unwrap_err().to_string(),
        );
    }

    #[test]
    fn test_version_display() {
        assert_eq!(
            "9.4.7.0",
            JRubyVersion::parse("9.4.7.0").unwrap().to_string()
        );

        assert_eq!(
            "10.1.0.0",
            JRubyVersion::parse("10.1.0.0").unwrap().to_string()
        );
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
