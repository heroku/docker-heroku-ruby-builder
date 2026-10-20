//! The HTML table format used inside the generated regions of the article.
//!
//! Every tag sits on its own line. [`Table::parse`] accepts only that exact layout, so a hand edit inside a generated
//! region fails loudly instead of being misread.

/// A table with one header row and any number of body rows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Table {
    pub headers: Vec<String>,
    pub rows: Vec<Row>,
}

/// A body row. `style` is the inline CSS of the `<tr>` tag.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub style: Option<String>,
    pub cells: Vec<String>,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("cannot parse generated table at line {line}: expected {expected} but found {found:?}")]
pub struct ParseError {
    line: usize,
    expected: &'static str,
    found: String,
}

impl Table {
    /// Renders the table with `\n` line endings.
    pub fn render(&self) -> String {
        let mut lines = vec![
            "<table>".to_string(),
            "<thead>".to_string(),
            "<tr>".to_string(),
        ];
        lines.extend(
            self.headers
                .iter()
                .map(|header| format!("<th>{header}</th>")),
        );
        lines.extend(["</tr>", "</thead>", "<tbody>"].map(String::from));
        for row in &self.rows {
            lines.push(match &row.style {
                Some(style) => format!(r#"<tr style="{style}">"#),
                None => "<tr>".to_string(),
            });
            lines.extend(row.cells.iter().map(|cell| format!("<td>{cell}</td>")));
            lines.push("</tr>".to_string());
        }
        lines.extend(["</tbody>", "</table>"].map(String::from));
        lines.join("\n")
    }

    /// Parses a table in the layout [`Table::render`] produces. Line endings may be `\n` or `\r\n`.
    pub fn parse(html: &str) -> Result<Table, ParseError> {
        let mut lines = Lines {
            inner: html.lines().map(str::trim).enumerate().peekable(),
        };
        lines.expect("<table>")?;
        lines.expect("<thead>")?;
        lines.expect("<tr>")?;
        let headers = lines.elements("<th>", "</th>");
        lines.expect("</tr>")?;
        lines.expect("</thead>")?;
        lines.expect("<tbody>")?;

        let mut rows = Vec::new();
        while let Some(style) = lines.row_start() {
            let cells = lines.elements("<td>", "</td>");
            lines.expect("</tr>")?;
            rows.push(Row { style, cells });
        }
        lines.expect("</tbody>")?;
        lines.expect("</table>")?;
        match lines.inner.next() {
            None => Ok(Table { headers, rows }),
            Some((index, found)) => Err(ParseError {
                line: index + 1,
                expected: "end of table",
                found: found.to_string(),
            }),
        }
    }
}

struct Lines<'a, I: Iterator<Item = (usize, &'a str)>> {
    inner: std::iter::Peekable<I>,
}

impl<'a, I: Iterator<Item = (usize, &'a str)>> Lines<'a, I> {
    fn expect(&mut self, tag: &'static str) -> Result<(), ParseError> {
        match self.inner.next() {
            Some((_, line)) if line == tag => Ok(()),
            Some((index, found)) => Err(ParseError {
                line: index + 1,
                expected: tag,
                found: found.to_string(),
            }),
            None => Err(ParseError {
                line: 0,
                expected: tag,
                found: "end of input".to_string(),
            }),
        }
    }

    /// Consumes consecutive `{open}text{close}` lines and returns their text.
    fn elements(&mut self, open: &str, close: &str) -> Vec<String> {
        let mut values = Vec::new();
        while let Some(value) = self
            .inner
            .peek()
            .and_then(|(_, line)| line.strip_prefix(open)?.strip_suffix(close))
        {
            values.push(value.to_string());
            self.inner.next();
        }
        values
    }

    /// Consumes a `<tr>` or `<tr style="...">` line, returning its style.
    fn row_start(&mut self) -> Option<Option<String>> {
        let (_, line) = self.inner.peek()?;
        let style = if *line == "<tr>" {
            None
        } else {
            Some(
                line.strip_prefix(r#"<tr style=""#)?
                    .strip_suffix(r#"">"#)?
                    .to_string(),
            )
        };
        self.inner.next();
        Some(style)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use indoc::indoc;
    use pretty_assertions::assert_eq;

    fn table() -> Table {
        Table {
            headers: vec!["Ruby series".to_string(), "heroku-24".to_string()],
            rows: vec![
                Row {
                    style: None,
                    cells: vec!["3.4".to_string(), "3.4.11".to_string()],
                },
                Row {
                    style: Some("color:#a94442;".to_string()),
                    cells: vec!["3.2 (EOL)".to_string(), "–".to_string()],
                },
            ],
        }
    }

    #[test]
    fn renders_one_tag_per_line() {
        assert_eq!(
            table().render(),
            indoc! {r#"
                <table>
                <thead>
                <tr>
                <th>Ruby series</th>
                <th>heroku-24</th>
                </tr>
                </thead>
                <tbody>
                <tr>
                <td>3.4</td>
                <td>3.4.11</td>
                </tr>
                <tr style="color:#a94442;">
                <td>3.2 (EOL)</td>
                <td>–</td>
                </tr>
                </tbody>
                </table>"#}
        );
    }

    #[test]
    fn parse_round_trips_render() {
        assert_eq!(Table::parse(&table().render()), Ok(table()));
    }

    #[test]
    fn parse_accepts_crlf() {
        let crlf = table().render().replace('\n', "\r\n");
        assert_eq!(Table::parse(&crlf), Ok(table()));
    }

    #[test]
    fn parse_accepts_a_table_without_rows() {
        let empty = Table {
            headers: vec!["a".to_string()],
            rows: Vec::new(),
        };
        assert_eq!(Table::parse(&empty.render()), Ok(empty));
    }

    #[test]
    fn parse_rejects_a_hand_edited_table() {
        let edited = table()
            .render()
            .replace("<td>3.4.11</td>", "<td><b>3.4.11</b>\n</td>");
        let error = Table::parse(&edited).unwrap_err();
        assert_eq!(
            error,
            ParseError {
                line: 11,
                expected: "</tr>",
                found: "<td><b>3.4.11</b>".to_string(),
            }
        );
    }

    #[test]
    fn parse_rejects_trailing_content() {
        let trailing = format!("{}\n<p>note</p>", table().render());
        assert!(Table::parse(&trailing).is_err());
    }

    #[test]
    fn parse_rejects_truncated_input() {
        assert!(Table::parse("<table>\n<thead>").is_err());
    }
}
