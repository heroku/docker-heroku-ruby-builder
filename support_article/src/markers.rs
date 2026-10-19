//! Regions of the article owned by this tool, delimited by HTML comments:
//!
//! ```text
//! <!-- BEGIN GENERATED ruby_versions_table -->
//! <table>...</table>
//! <!-- END GENERATED ruby_versions_table -->
//! ```
//!
//! Dev Center keeps comments in the stored Markdown and hides them on the rendered page.

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum MarkerError {
    #[error("expected exactly one `{marker}` in the article but found {count}")]
    Count { marker: String, count: usize },

    #[error("`{end}` appears before `{begin}` in the article")]
    OutOfOrder { begin: String, end: String },
}

fn markers(name: &str) -> (String, String) {
    (
        format!("<!-- BEGIN GENERATED {name} -->"),
        format!("<!-- END GENERATED {name} -->"),
    )
}

fn find_once(content: &str, marker: &str) -> Result<usize, MarkerError> {
    let mut matches = content.match_indices(marker).map(|(index, _)| index);
    match (matches.next(), matches.count()) {
        (Some(index), 0) => Ok(index),
        (first, rest) => Err(MarkerError::Count {
            marker: marker.to_string(),
            count: usize::from(first.is_some()) + rest,
        }),
    }
}

/// Byte range of the text strictly between the `name` markers.
fn region_range(content: &str, name: &str) -> Result<std::ops::Range<usize>, MarkerError> {
    let (begin, end) = markers(name);
    let start = find_once(content, &begin)? + begin.len();
    let finish = find_once(content, &end)?;
    if finish < start {
        return Err(MarkerError::OutOfOrder { begin, end });
    }
    Ok(start..finish)
}

/// The text between the `name` markers, trimmed of surrounding whitespace.
pub fn generated_region<'a>(content: &'a str, name: &str) -> Result<&'a str, MarkerError> {
    Ok(content[region_range(content, name)?].trim())
}

/// Replaces the text between the `name` markers with `generated`, leaving everything else byte for byte.
///
/// `generated` uses `\n` line endings. They are converted to `\r\n` when the article uses CRLF.
pub fn replace_generated(
    content: &str,
    name: &str,
    generated: &str,
) -> Result<String, MarkerError> {
    let range = region_range(content, name)?;
    let newline = if content.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    let generated = generated.replace('\n', newline);
    Ok(format!(
        "{before}{newline}{generated}{newline}{after}",
        before = &content[..range.start],
        after = &content[range.end..],
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    const ARTICLE: &str =
        "intro\r\n<!-- BEGIN GENERATED t -->\r\nold\r\n<!-- END GENERATED t -->\r\noutro\r\n";

    #[test]
    fn replaces_only_the_named_region_and_keeps_crlf() {
        assert_eq!(
            replace_generated(ARTICLE, "t", "a\nb").unwrap(),
            "intro\r\n<!-- BEGIN GENERATED t -->\r\na\r\nb\r\n<!-- END GENERATED t -->\r\noutro\r\n"
        );
    }

    #[test]
    fn keeps_lf_articles_lf() {
        let article = ARTICLE.replace("\r\n", "\n");
        assert_eq!(
            replace_generated(&article, "t", "a\nb").unwrap(),
            "intro\n<!-- BEGIN GENERATED t -->\na\nb\n<!-- END GENERATED t -->\noutro\n"
        );
    }

    #[test]
    fn replacing_with_the_current_region_is_a_no_op() {
        assert_eq!(replace_generated(ARTICLE, "t", "old").unwrap(), ARTICLE);
    }

    #[test]
    fn reads_the_region_between_markers() {
        assert_eq!(generated_region(ARTICLE, "t"), Ok("old"));
    }

    #[test]
    fn markers_for_other_names_do_not_match() {
        assert_eq!(
            generated_region(ARTICLE, "other"),
            Err(MarkerError::Count {
                marker: "<!-- BEGIN GENERATED other -->".to_string(),
                count: 0
            })
        );
    }

    #[test]
    fn errors_on_a_missing_end_marker() {
        let article = ARTICLE.replace("<!-- END GENERATED t -->", "");
        assert_eq!(
            replace_generated(&article, "t", "new"),
            Err(MarkerError::Count {
                marker: "<!-- END GENERATED t -->".to_string(),
                count: 0
            })
        );
    }

    #[test]
    fn errors_on_a_duplicated_marker() {
        let article = format!("{ARTICLE}<!-- BEGIN GENERATED t -->\r\n");
        assert_eq!(
            replace_generated(&article, "t", "new"),
            Err(MarkerError::Count {
                marker: "<!-- BEGIN GENERATED t -->".to_string(),
                count: 2
            })
        );
    }

    #[test]
    fn errors_on_markers_out_of_order() {
        let article = "<!-- END GENERATED t -->\r\nold\r\n<!-- BEGIN GENERATED t -->";
        assert!(matches!(
            replace_generated(article, "t", "new"),
            Err(MarkerError::OutOfOrder { .. })
        ));
    }
}
