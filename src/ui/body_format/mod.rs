//! Display-only formatting of decoded body previews: format detection, the
//! Pretty and Raw view modes, and format-specific pretty printers that emit
//! classified tokens for syntax highlighting.
//!
//! Each format lives in its own module and only turns text into a [`Styled`]
//! token stream; [`highlight`] maps the shared [`TokenKind`]s to theme colors.
//! The result never feeds back into forwarding or the full log.

mod css;
pub(in crate::ui) mod form;
pub(in crate::ui) mod highlight;
mod javascript;
mod json;
mod json_tree;
mod markup;
pub(in crate::ui) mod tree;
pub(in crate::ui) mod xml;

/// Body syntax detected from `Content-Type`.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(in crate::ui) enum BodyFormat {
    Json,
    Css,
    Form,
    Markup,
    Xml,
    JavaScript,
    Text,
}

impl BodyFormat {
    /// Detects the format from a `Content-Type` value, ignoring parameters
    /// and case. Unknown or missing types are plain text.
    pub(in crate::ui) fn from_content_type(content_type: Option<&str>) -> Self {
        let Some(value) = content_type else {
            return Self::Text;
        };
        let essence = value
            .split(';')
            .next()
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase();
        let subtype = essence.split_once('/').map_or("", |(_, subtype)| subtype);
        if subtype == "json" || subtype.ends_with("+json") {
            Self::Json
        } else if essence == "text/css" {
            Self::Css
        } else if essence == "application/x-www-form-urlencoded" {
            Self::Form
        } else if matches!(essence.as_str(), "text/html" | "application/xhtml+xml") {
            Self::Markup
        } else if matches!(essence.as_str(), "application/xml" | "text/xml")
            || subtype.ends_with("+xml")
        {
            Self::Xml
        } else if matches!(
            essence.as_str(),
            "text/javascript"
                | "application/javascript"
                | "application/x-javascript"
                | "application/ecmascript"
                | "text/ecmascript"
        ) {
            Self::JavaScript
        } else {
            Self::Text
        }
    }

    pub(in crate::ui) const fn label(self) -> &'static str {
        match self {
            Self::Json => "JSON",
            Self::Css => "CSS",
            Self::Form => "Form",
            Self::Markup => "HTML",
            Self::Xml => "XML",
            Self::JavaScript => "JavaScript",
            Self::Text => "Text",
        }
    }
}

/// How a body preview is shown. Formats without a tree show Tree as Pretty.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub(in crate::ui) enum ViewMode {
    #[default]
    Pretty,
    Raw,
    Tree,
}

impl ViewMode {
    pub(in crate::ui) const ALL: [Self; 3] = [Self::Pretty, Self::Raw, Self::Tree];

    pub(in crate::ui) const fn label(self) -> &'static str {
        match self {
            Self::Pretty => "Pretty",
            Self::Raw => "Raw",
            Self::Tree => "Tree",
        }
    }
}

/// Syntactic role of a piece of displayed text, shared by every format.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(in crate::ui) enum TokenKind {
    Text,
    Key,
    String,
    Number,
    Literal,
    Punctuation,
    Tag,
    AttributeName,
    AttributeValue,
    Comment,
    /// XML namespace prefix, such as `soap` in `soap:Body`.
    Namespace,
    /// XML entity or character reference, such as `&amp;`.
    Entity,
    /// XML `CDATA` section with its delimiters.
    CData,
    /// XML declaration, processing instruction, or `DOCTYPE`.
    Instruction,
}

/// Text with every byte assigned to a [`TokenKind`]; adjacent pieces of the
/// same kind are merged.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(in crate::ui) struct Styled {
    text: String,
    spans: Vec<(std::ops::Range<usize>, TokenKind)>,
}

impl Styled {
    pub(in crate::ui) fn plain(text: &str) -> Self {
        let mut styled = Self::default();
        styled.push(text, TokenKind::Text);
        styled
    }

    pub(in crate::ui) fn push(&mut self, piece: &str, kind: TokenKind) {
        if piece.is_empty() {
            return;
        }
        let start = self.text.len();
        self.text.push_str(piece);
        let end = self.text.len();
        match self.spans.last_mut() {
            Some((range, last)) if *last == kind => range.end = end,
            _ => self.spans.push((start..end, kind)),
        }
    }

    pub(in crate::ui) fn text(&self) -> &str {
        &self.text
    }

    /// Iterates over the text pieces and their kinds in order.
    pub(in crate::ui) fn pieces(&self) -> impl Iterator<Item = (&str, TokenKind)> {
        self.spans
            .iter()
            .filter_map(|(range, kind)| Some((self.text.get(range.clone())?, *kind)))
    }

    fn newline(&mut self, depth: usize) {
        self.push("\n", TokenKind::Text);
        self.push(&"  ".repeat(depth), TokenKind::Text);
    }
}

/// Body text prepared for one view mode.
#[derive(Debug, Eq, PartialEq)]
pub(in crate::ui) struct Formatted {
    pub(in crate::ui) styled: Styled,
    /// Why Pretty fell back to the original text, if it did.
    pub(in crate::ui) fallback: Option<String>,
    /// What the shown text leaves out, such as the rest of malformed XML.
    pub(in crate::ui) warning: Option<String>,
    /// Neutral information about the shown text, such as the source map a
    /// script points to.
    pub(in crate::ui) note: Option<String>,
}

/// Formats `text` for `mode`. Pretty falls back to the original text with a
/// reason when the format has no pretty printer or the text cannot be read
/// as that format; truncated input is formatted as far as it goes.
/// Tree has no text of its own, so it is formatted as Pretty here.
pub(in crate::ui) fn format(format: BodyFormat, mode: ViewMode, text: &str) -> Formatted {
    let pretty = match (mode, format) {
        (ViewMode::Raw, _) | (ViewMode::Pretty | ViewMode::Tree, BodyFormat::Text) => {
            return raw(text, None);
        }
        (ViewMode::Pretty | ViewMode::Tree, BodyFormat::Json) => json::pretty(text).map(complete),
        (ViewMode::Pretty | ViewMode::Tree, BodyFormat::Form) => Ok(complete(form::pretty(text))),
        (ViewMode::Pretty | ViewMode::Tree, BodyFormat::Markup) => {
            markup::pretty(text).map(complete)
        }
        (ViewMode::Pretty | ViewMode::Tree, BodyFormat::Css) => return css::pretty(text),
        (ViewMode::Pretty | ViewMode::Tree, BodyFormat::Xml) => xml::pretty(text),
        (ViewMode::Pretty | ViewMode::Tree, BodyFormat::JavaScript) => {
            return javascript_pretty(text);
        }
    };
    match pretty {
        Ok((styled, warning)) => Formatted {
            styled,
            fallback: None,
            warning,
            note: None,
        },
        Err(reason) => unformatted(format, text, &reason),
    }
}

/// The original text with the reason Pretty could not format it.
fn unformatted(format: BodyFormat, text: &str, reason: &str) -> Formatted {
    raw(
        text,
        Some(format!(
            "Showing raw text: could not format as {}: {reason}.",
            format.label()
        )),
    )
}

const fn complete(styled: Styled) -> (Styled, Option<String>) {
    (styled, None)
}

fn javascript_pretty(text: &str) -> Formatted {
    match javascript::pretty(text) {
        Ok(pretty) => Formatted {
            styled: pretty.styled,
            fallback: None,
            warning: pretty.warning,
            note: pretty.note,
        },
        Err(reason) => unformatted(BodyFormat::JavaScript, text, &reason),
    }
}

fn raw(text: &str, fallback: Option<String>) -> Formatted {
    Formatted {
        styled: Styled::plain(text),
        fallback,
        warning: None,
        note: None,
    }
}

/// Splits `text` after the longest prefix whose characters match `accept`.
fn split_while(text: &str, accept: impl Fn(char) -> bool) -> (&str, &str) {
    text.split_at(text.find(|c| !accept(c)).unwrap_or(text.len()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_formats_from_content_type() {
        for (value, expected) in [
            (Some("application/json"), BodyFormat::Json),
            (Some("Text/CSS; charset=utf-8"), BodyFormat::Css),
            (Some("Application/JSON; charset=utf-8"), BodyFormat::Json),
            (Some("application/problem+json"), BodyFormat::Json),
            (Some("application/x-www-form-urlencoded"), BodyFormat::Form),
            (Some("text/html; charset=UTF-8"), BodyFormat::Markup),
            (Some("application/xhtml+xml"), BodyFormat::Markup),
            (Some("application/xml"), BodyFormat::Xml),
            (Some("text/xml; charset=utf-8"), BodyFormat::Xml),
            (Some("application/atom+xml"), BodyFormat::Xml),
            (Some("application/soap+xml"), BodyFormat::Xml),
            (Some("image/svg+xml"), BodyFormat::Xml),
            (Some("text/plain"), BodyFormat::Text),
            (Some("application/javascript"), BodyFormat::JavaScript),
            (
                Some("text/javascript; charset=utf-8"),
                BodyFormat::JavaScript,
            ),
            (Some("application/x-javascript"), BodyFormat::JavaScript),
            (Some("application/ecmascript"), BodyFormat::JavaScript),
            (Some("Text/ECMAScript"), BodyFormat::JavaScript),
            (Some("jsonish"), BodyFormat::Text),
            (None, BodyFormat::Text),
        ] {
            assert_eq!(BodyFormat::from_content_type(value), expected, "{value:?}");
        }
    }

    #[test]
    fn raw_keeps_original_text() {
        let text = "{\"a\":1}";
        let formatted = format(BodyFormat::Json, ViewMode::Raw, text);
        assert_eq!(formatted.styled.text(), text);
        assert_eq!(formatted.fallback, None);
    }

    #[test]
    fn pretty_falls_back_to_raw_with_a_reason() {
        let text = "not json at all";
        let formatted = format(BodyFormat::Json, ViewMode::Pretty, text);
        assert_eq!(formatted.styled.text(), text);
        assert!(
            formatted
                .fallback
                .is_some_and(|reason| reason.contains("JSON"))
        );
    }

    #[test]
    fn merges_adjacent_pieces_of_the_same_kind() {
        let mut styled = Styled::default();
        styled.push("a", TokenKind::Key);
        styled.push("b", TokenKind::Key);
        styled.push("", TokenKind::Number);
        styled.push("1", TokenKind::Number);
        assert_eq!(
            styled.pieces().collect::<Vec<_>>(),
            vec![("ab", TokenKind::Key), ("1", TokenKind::Number)]
        );
    }
}
