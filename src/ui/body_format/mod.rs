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
mod tag;
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

    /// What this format supports, declared in [`FormatSpec`].
    pub(in crate::ui) const fn spec(self) -> &'static FormatSpec {
        match self {
            Self::Json => &FormatSpec {
                label: "JSON",
                pretty: Some(|text| json::pretty(text).map(Formatted::plain)),
                tree: Some(json_tree::build),
                numbered: false,
            },
            Self::Css => &FormatSpec {
                label: "CSS",
                pretty: Some(css::pretty),
                tree: None,
                numbered: true,
            },
            Self::Form => &FormatSpec {
                label: "Form",
                pretty: Some(|text| Ok(Formatted::plain(form::pretty(text)))),
                tree: None,
                numbered: false,
            },
            Self::Markup => &FormatSpec {
                label: "HTML",
                pretty: Some(|text| markup::pretty(text).map(Formatted::plain)),
                tree: None,
                numbered: false,
            },
            Self::Xml => &FormatSpec {
                label: "XML",
                pretty: Some(xml::pretty),
                tree: Some(xml::tree::build),
                numbered: false,
            },
            Self::JavaScript => &FormatSpec {
                label: "JavaScript",
                pretty: Some(javascript::pretty),
                tree: None,
                numbered: true,
            },
            Self::Text => &FormatSpec {
                label: "Text",
                pretty: None,
                tree: None,
                numbered: false,
            },
        }
    }

    pub(in crate::ui) const fn label(self) -> &'static str {
        self.spec().label
    }
}

/// Reads text as a format and returns it formatted, or why it could not.
pub(in crate::ui) type PrettyFn = fn(&str) -> Result<Formatted, String>;

/// Reads text as a format and returns its tree, or why it could not.
pub(in crate::ui) type TreeFn = fn(&str) -> Result<tree::TreeModel, String>;

/// Everything the body view needs to know about one format. Adding a format
/// means adding its [`BodyFormat::spec`] entry and its formatter module.
#[derive(Debug)]
pub(in crate::ui) struct FormatSpec {
    /// Name shown next to the view mode.
    pub(in crate::ui) label: &'static str,
    /// Pretty printer; without one Pretty shows the original text.
    pub(in crate::ui) pretty: Option<PrettyFn>,
    /// Tree builder; without one Tree shows Pretty with a notice.
    pub(in crate::ui) tree: Option<TreeFn>,
    /// Whether Pretty and Raw are shown with line numbers.
    pub(in crate::ui) numbered: bool,
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

/// How a [`Notice`] is shown.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::ui) enum NoticeLevel {
    /// Neutral information, such as the decoding used or a source map URL.
    Info,
    /// Something the shown text leaves out or could not do.
    Warning,
}

/// One line of information shown above a body.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::ui) struct Notice {
    pub(in crate::ui) level: NoticeLevel,
    pub(in crate::ui) text: String,
}

impl Notice {
    pub(in crate::ui) fn info(text: impl Into<String>) -> Self {
        Self {
            level: NoticeLevel::Info,
            text: text.into(),
        }
    }

    pub(in crate::ui) fn warning(text: impl Into<String>) -> Self {
        Self {
            level: NoticeLevel::Warning,
            text: text.into(),
        }
    }
}

/// Body text prepared for one view mode, with what the reader should know
/// about it in display order.
#[derive(Debug, Eq, PartialEq)]
pub(in crate::ui) struct Formatted {
    pub(in crate::ui) styled: Styled,
    pub(in crate::ui) notices: Vec<Notice>,
}

impl Formatted {
    /// Formatted text with nothing to point out.
    pub(in crate::ui) const fn plain(styled: Styled) -> Self {
        Self {
            styled,
            notices: Vec::new(),
        }
    }
}

/// Formats `text` for `mode`. Pretty falls back to the original text with a
/// reason when the format has no pretty printer or the text cannot be read
/// as that format; truncated input is formatted as far as it goes.
/// Tree has no text of its own, so it is formatted as Pretty here.
pub(in crate::ui) fn format(format: BodyFormat, mode: ViewMode, text: &str) -> Formatted {
    let spec = format.spec();
    let pretty = match (mode, spec.pretty) {
        (ViewMode::Raw, _) | (_, None) => return Formatted::plain(Styled::plain(text)),
        (ViewMode::Pretty | ViewMode::Tree, Some(pretty)) => pretty,
    };
    pretty(text).unwrap_or_else(|reason| Formatted {
        styled: Styled::plain(text),
        notices: vec![Notice::warning(format!(
            "Showing raw text: could not format as {}: {reason}.",
            spec.label
        ))],
    })
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
        assert_eq!(formatted.notices, []);
    }

    #[test]
    fn pretty_falls_back_to_raw_with_a_reason() {
        let text = "not json at all";
        let formatted = format(BodyFormat::Json, ViewMode::Pretty, text);
        assert_eq!(formatted.styled.text(), text);
        assert!(matches!(
            formatted.notices.as_slice(),
            [Notice { level: NoticeLevel::Warning, text }] if text.contains("JSON")
        ));
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
