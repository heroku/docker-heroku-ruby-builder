use reqwest::Url;
use reqwest::header::{HeaderMap, LINK, ToStrError};
use winnow::{
    Parser, Result,
    ascii::space0,
    combinator::{alt, delimited, preceded, repeat, separated},
    token::{take_until, take_while},
};

#[derive(thiserror::Error, Debug)]
pub enum GithubHeaderError {
    #[error("Link header contains non-ASCII characters: {0}")]
    NonAsciiChars(ToStrError),

    #[error("Malformed Link header `{header}`: {message}")]
    Malformed { header: String, message: String },

    #[error("Cannot parse URL `{raw}` from Link header `{header}`: {source}")]
    CannotParseUrl {
        raw: String,
        header: String,
        source: url::ParseError,
    },
}

/// A single GitHub pagination link, identified by its `rel` value.
///
/// GitHub's REST API only ever emits these four relations, each pointing at a
/// page of results. Entries with any other `rel` are ignored (see
/// [`pagination_links`]).
#[derive(Debug, PartialEq, Eq)]
pub enum PageLink {
    First(Url),
    Prev(Url),
    Next(Url),
    Last(Url),
}

impl PageLink {
    /// The target URL, regardless of which relation this is.
    #[must_use]
    pub fn url(&self) -> &Url {
        match self {
            PageLink::First(url)
            | PageLink::Prev(url)
            | PageLink::Next(url)
            | PageLink::Last(url) => url,
        }
    }
}

/// Parse every recognized pagination link from a GitHub `Link` header.
///
/// Returns an empty `Vec` when the header is absent (GitHub omits it entirely
/// when all results fit on one page). Entries whose `rel` is not one of
/// GitHub's four documented values are skipped. Every recognized entry's URL is
/// parsed, so a malformed URL anywhere in the header surfaces as an error.
pub fn pagination_links(headers: &HeaderMap) -> Result<Vec<PageLink>, GithubHeaderError> {
    let Some(value) = headers.get(LINK) else {
        return Ok(Vec::new());
    };
    let header = value.to_str().map_err(GithubHeaderError::NonAsciiChars)?;

    let entries =
        link_header
            .parse(header.trim())
            .map_err(|error| GithubHeaderError::Malformed {
                header: header.to_owned(),
                message: error.to_string(),
            })?;

    let mut links = Vec::new();
    for entry in entries {
        if let Some(link) = entry.into_page_link(header)? {
            links.push(link);
        }
    }
    Ok(links)
}

/// Extract the `rel="next"` URL from a GitHub `Link` header, if present.
///
/// GitHub omits the `next` entry on the last page (and omits the header entirely
/// when all results fit on one page), so the absence of a next link is the
/// authoritative end-of-pagination signal and is reported as `Ok(None)` rather
/// than an error.
pub fn next_page(headers: &HeaderMap) -> Result<Option<Url>, GithubHeaderError> {
    Ok(pagination_links(headers)?
        .into_iter()
        .find_map(|link| match link {
            PageLink::Next(url) => Some(url),
            _ => None,
        }))
}

/// A single comma-separated entry of a `Link` header: a target URL plus its
/// trailing `;`-separated parameters (e.g. `rel`, `type`).
#[derive(Debug, PartialEq, Eq)]
struct LinkEntry<'a> {
    url: &'a str,
    params: Vec<(&'a str, &'a str)>,
}

impl LinkEntry<'_> {
    /// Map a raw parsed entry onto a typed [`PageLink`].
    ///
    /// `Ok(None)` means the entry carries no `rel`, or a `rel` GitHub never
    /// emits for pagination; both are silently skipped rather than treated as
    /// errors. `header` is only used to enrich [`GithubHeaderError::CannotParseUrl`].
    fn into_page_link(self, header: &str) -> Result<Option<PageLink>, GithubHeaderError> {
        let Some((_, rel)) = self.params.iter().find(|(name, _)| *name == "rel") else {
            return Ok(None);
        };
        let variant = match *rel {
            "first" => PageLink::First,
            "prev" => PageLink::Prev,
            "next" => PageLink::Next,
            "last" => PageLink::Last,
            _ => return Ok(None),
        };
        let url = Url::parse(self.url).map_err(|source| GithubHeaderError::CannotParseUrl {
            raw: self.url.to_owned(),
            header: header.to_owned(),
            source,
        })?;
        Ok(Some(variant(url)))
    }
}

/// `link-header = link-entry *( "," link-entry )`
fn link_header<'a>(input: &mut &'a str) -> Result<Vec<LinkEntry<'a>>> {
    separated(1.., link_entry, (space0, ',', space0)).parse_next(input)
}

/// `link-entry = "<" URI ">" *( ";" param )`
fn link_entry<'a>(input: &mut &'a str) -> Result<LinkEntry<'a>> {
    let url = delimited('<', take_until(1.., '>'), '>').parse_next(input)?;
    let params = repeat(0.., preceded((space0, ';', space0), param)).parse_next(input)?;
    Ok(LinkEntry { url, params })
}

/// `param = token "=" ( quoted-string / token )`
fn param<'a>(input: &mut &'a str) -> Result<(&'a str, &'a str)> {
    let name = take_while(1.., |c: char| {
        c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')
    })
    .parse_next(input)?;
    let _ = (space0, '=', space0).parse_next(input)?;
    let value = alt((quoted_string, token)).parse_next(input)?;
    Ok((name, value))
}

/// `quoted-string = DQUOTE *( <any char except DQUOTE> ) DQUOTE`
fn quoted_string<'a>(input: &mut &'a str) -> Result<&'a str> {
    delimited('"', take_until(0.., '"'), '"').parse_next(input)
}

/// `token = 1*( <any char except "," ";" or whitespace> )`
fn token<'a>(input: &mut &'a str) -> Result<&'a str> {
    take_while(1.., |c: char| !matches!(c, ',' | ';') && !c.is_whitespace()).parse_next(input)
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::header::HeaderValue;

    fn headers_with_link(value: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(LINK, HeaderValue::from_str(value).unwrap());
        headers
    }

    #[test]
    fn finds_next_among_multiple_entries() {
        let header = r#"<https://api.github.com/repos/jruby/jruby/releases?per_page=100&page=2>; rel="next", <https://api.github.com/repos/jruby/jruby/releases?per_page=100&page=5>; rel="last""#;
        let next = next_page(&headers_with_link(header)).unwrap();
        assert_eq!(
            next.map(|url| url.to_string()),
            Some(
                "https://api.github.com/repos/jruby/jruby/releases?per_page=100&page=2".to_owned()
            )
        );
    }

    #[test]
    fn next_can_appear_after_prev() {
        let header = r#"<https://api.github.com/x?page=1>; rel="prev", <https://api.github.com/x?page=3>; rel="next""#;
        let next = next_page(&headers_with_link(header)).unwrap();
        assert_eq!(
            next.map(|url| url.to_string()),
            Some("https://api.github.com/x?page=3".to_owned())
        );
    }

    #[test]
    fn last_page_has_no_next() {
        let header = r#"<https://api.github.com/x?page=1>; rel="prev", <https://api.github.com/x?page=1>; rel="first""#;
        assert_eq!(next_page(&headers_with_link(header)).unwrap(), None);
    }

    #[test]
    fn missing_header_is_end_of_pagination() {
        assert_eq!(next_page(&HeaderMap::new()).unwrap(), None);
    }

    #[test]
    fn parses_all_four_relations_as_enum() {
        let header = r#"<https://api.github.com/x?page=1>; rel="prev", <https://api.github.com/x?page=3>; rel="next", <https://api.github.com/x?page=1>; rel="first", <https://api.github.com/x?page=9>; rel="last""#;
        let links = pagination_links(&headers_with_link(header)).unwrap();
        assert_eq!(
            links,
            vec![
                PageLink::Prev(Url::parse("https://api.github.com/x?page=1").unwrap()),
                PageLink::Next(Url::parse("https://api.github.com/x?page=3").unwrap()),
                PageLink::First(Url::parse("https://api.github.com/x?page=1").unwrap()),
                PageLink::Last(Url::parse("https://api.github.com/x?page=9").unwrap()),
            ]
        );
    }

    #[test]
    fn unknown_rel_is_skipped() {
        let header = r#"<https://example.com/spec>; rel="describedby", <https://example.com/p2>; rel="next""#;
        let links = pagination_links(&headers_with_link(header)).unwrap();
        assert_eq!(
            links,
            vec![PageLink::Next(
                Url::parse("https://example.com/p2").unwrap()
            )]
        );
    }

    #[test]
    fn parses_entry_with_extra_params() {
        let mut input = r#"<https://example.com/page2>; rel="next"; type="text/html""#;
        let entry = link_entry(&mut input).unwrap();
        assert_eq!(
            entry,
            LinkEntry {
                url: "https://example.com/page2",
                params: vec![("rel", "next"), ("type", "text/html")],
            }
        );
    }
}
