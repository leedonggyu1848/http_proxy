//! Display-only formatting of decoded body previews: format detection, the
//! Pretty and Raw view modes, and format-specific pretty printers that emit
//! classified tokens for syntax highlighting.
//!
//! Each format lives in its own module and only turns text into a [`Styled`]
//! token stream; [`highlight`] maps the shared [`TokenKind`]s to theme colors.
//! The result never feeds back into forwarding or the full log.

pub(in crate::ui) mod form;
pub(in crate::ui) mod highlight;
mod json;
mod markup;

/// Body syntax detected from `Content-Type`.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(in crate::ui) enum BodyFormat {
    Json,
    Form,
    Markup,
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
        } else if essence == "application/x-www-form-urlencoded" {
            Self::Form
        } else if matches!(
            essence.as_str(),
            "text/html" | "application/xhtml+xml" | "application/xml" | "text/xml"
        ) || subtype.ends_with("+xml")
        {
            Self::Markup
        } else {
            Self::Text
        }
    }

    pub(in crate::ui) const fn label(self) -> &'static str {
        match self {
            Self::Json => "JSON",
            Self::Form => "Form",
            Self::Markup => "HTML/XML",
            Self::Text => "Text",
        }
    }
}

/// How a body preview is shown. New views (such as a tree) are added here.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub(in crate::ui) enum ViewMode {
    #[default]
    Pretty,
    Raw,
}

impl ViewMode {
    pub(in crate::ui) const ALL: [Self; 2] = [Self::Pretty, Self::Raw];

    pub(in crate::ui) const fn label(self) -> &'static str {
        match self {
            Self::Pretty => "Pretty",
            Self::Raw => "Raw",
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
}

/// Formats `text` for `mode`. Pretty falls back to the original text with a
/// reason when the format has no pretty printer or the text cannot be read
/// as that format; truncated input is formatted as far as it goes.
pub(in crate::ui) fn format(format: BodyFormat, mode: ViewMode, text: &str) -> Formatted {
    let pretty = match (mode, format) {
        (ViewMode::Raw, _) | (ViewMode::Pretty, BodyFormat::Text) => return raw(text, None),
        (ViewMode::Pretty, BodyFormat::Json) => json::pretty(text),
        (ViewMode::Pretty, BodyFormat::Form) => Ok(form::pretty(text)),
        (ViewMode::Pretty, BodyFormat::Markup) => markup::pretty(text),
    };
    match pretty {
        Ok(styled) => Formatted {
            styled,
            fallback: None,
        },
        Err(reason) => raw(
            text,
            Some(format!(
                "Showing raw text: could not format as {}: {reason}.",
                format.label()
            )),
        ),
    }
}

fn raw(text: &str, fallback: Option<String>) -> Formatted {
    Formatted {
        styled: Styled::plain(text),
        fallback,
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
            (Some("Application/JSON; charset=utf-8"), BodyFormat::Json),
            (Some("application/problem+json"), BodyFormat::Json),
            (Some("application/x-www-form-urlencoded"), BodyFormat::Form),
            (Some("text/html; charset=UTF-8"), BodyFormat::Markup),
            (Some("application/xml"), BodyFormat::Markup),
            (Some("text/xml"), BodyFormat::Markup),
            (Some("application/atom+xml"), BodyFormat::Markup),
            (Some("text/plain"), BodyFormat::Text),
            (Some("application/javascript"), BodyFormat::Text),
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
