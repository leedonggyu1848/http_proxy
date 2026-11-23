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

use http_proxy::MediaType;

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
    /// Detects the format from the body's media type. Unknown or missing
    /// types are plain text.
    pub(in crate::ui) fn from_media_type(media_type: Option<&MediaType>) -> Self {
        let Some(media_type) = media_type else {
            return Self::Text;
        };
        match media_type.essence() {
            _ if media_type.is_json() => Self::Json,
            "text/css" => Self::Css,
            "application/x-www-form-urlencoded" => Self::Form,
            "text/html" | "application/xhtml+xml" => Self::Markup,
            "application/xml" | "text/xml" => Self::Xml,
            _ if media_type.has_xml_suffix() => Self::Xml,
            "text/javascript"
            | "application/javascript"
            | "application/x-javascript"
            | "application/ecmascript"
            | "text/ecmascript" => Self::JavaScript,
            _ => Self::Text,
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

/// Visual-only indentation applied at the start of a continuation line
/// (`offset` immediately follows `\n`) without inserting whitespace into
/// [`Styled::text`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::ui) struct LineIndent {
    /// Byte offset in [`Styled::text`] where the indented line starts.
    pub(in crate::ui) offset: usize,
    /// Reference prefix whose rendered width in the active monospace font sets
    /// the visual indentation of the line.
    pub(in crate::ui) align_to: String,
}

/// Text with every byte assigned to a [`TokenKind`]; adjacent pieces of the
/// same kind are merged.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(in crate::ui) struct Styled {
    text: String,
    spans: Vec<(std::ops::Range<usize>, TokenKind)>,
    indents: Vec<LineIndent>,
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

    /// Appends `piece` and aligns every non-empty continuation line inside it
    /// visually to `align_to`, without adding spaces to [`Self::text`].
    pub(super) fn push_multiline(&mut self, piece: &str, kind: TokenKind, align_to: &str) {
        let start = self.text.len();
        self.push(piece, kind);
        if align_to.is_empty() || !piece.contains('\n') {
            return;
        }
        for (nl, _) in piece.match_indices('\n') {
            let next = nl + 1;
            if let Some(rest) = piece.get(next..)
                && !rest.is_empty()
                && !is_empty_line_start(rest)
            {
                self.add_indent(start + next, align_to);
            }
        }
    }

    /// Appends a quoted/backtick-delimited value and aligns any internal
    /// continuation lines to the start of the value content (just past the
    /// opening delimiter).
    pub(super) fn push_quoted_multiline(&mut self, piece: &str, kind: TokenKind) {
        if !piece.contains('\n') {
            self.push(piece, kind);
            return;
        }
        let mut align = self.visual_current_line();
        if let Some(open @ ('"' | '\'' | '`')) = piece.chars().next() {
            align.push(open);
        }
        self.push_multiline(piece, kind, &align);
    }

    /// Records a visual indent for the current position when it starts a new
    /// line immediately after `\n`.
    pub(super) fn indent_line_if_at_start(&mut self, align_to: &str) {
        if self.text.ends_with('\n') {
            self.add_indent(self.text.len(), align_to);
        }
    }

    /// Appends the trailing horizontal-whitespace slice before a closing tag
    /// verbatim while visually padding any shortfall below `depth * 2` spaces
    /// so the closing tag never protrudes into an outer nesting column.
    pub(super) fn push_closing_tail(&mut self, tail: &str, depth: usize) {
        if tail.is_empty() {
            return;
        }
        if !tail.contains('\t') {
            let spaces = tail.bytes().filter(|&b| b == b' ').count();
            let missing = (depth * 2).saturating_sub(spaces);
            if missing > 0 {
                self.indent_line_if_at_start(&" ".repeat(missing));
            }
        }
        self.push(tail, TokenKind::Text);
    }

    fn add_indent(&mut self, offset: usize, align_to: &str) {
        if align_to.is_empty() {
            return;
        }
        if self.indents.last().is_none_or(|last| last.offset < offset) {
            self.indents.push(LineIndent {
                offset,
                align_to: align_to.to_owned(),
            });
        }
    }

    /// Text on the current line (since the last `\n`), combined with any visual
    /// indent already active on that line.
    pub(super) fn visual_current_line(&self) -> String {
        let line_start = self.text.rfind('\n').map_or(0, |index| index + 1);
        let current = self.text.get(line_start..).unwrap_or_default();
        match self.indents.last() {
            Some(indent) if indent.offset == line_start => {
                let mut combined = String::with_capacity(indent.align_to.len() + current.len());
                combined.push_str(&indent.align_to);
                combined.push_str(current);
                combined
            }
            _ => current.to_owned(),
        }
    }

    pub(in crate::ui) fn text(&self) -> &str {
        &self.text
    }

    pub(in crate::ui) fn indents(&self) -> &[LineIndent] {
        &self.indents
    }

    /// Iterates over the byte ranges and kinds in order.
    pub(in crate::ui) fn span_ranges(
        &self,
    ) -> impl Iterator<Item = (std::ops::Range<usize>, TokenKind)> + '_ {
        self.spans
            .iter()
            .map(|(range, kind)| (range.clone(), *kind))
    }

    /// Iterates over the text pieces and their kinds in order.
    #[cfg(test)]
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

fn is_empty_line_start(rest: &str) -> bool {
    rest.starts_with('\n') || rest.starts_with("\r\n")
}

/// Whether `text` has a line break whose preceding prefix contains only
/// horizontal whitespace (or is empty), meaning the visible value begins on a
/// following line rather than the opening line.
pub(super) fn starts_with_blank_line(text: &str) -> bool {
    text.split_once('\n')
        .is_some_and(|(first, _)| first.trim_matches([' ', '\t', '\r']).is_empty())
}

/// Splits off a trailing line that contains only horizontal whitespace after
/// the last `\n` (typically the original indentation before a closing tag).
pub(super) fn split_trailing_blank_line(text: &str) -> (&str, &str) {
    let Some(nl) = text.rfind('\n') else {
        return (text, "");
    };
    let after = text.get(nl + 1..).unwrap_or_default();
    if !after.is_empty() && after.trim_matches([' ', '\t', '\r']).is_empty() {
        text.split_at(nl + 1)
    } else {
        (text, "")
    }
}

/// Visual alignment target for continuation lines of a delimited comment such
/// as `<!-- … -->` or `/* … */`.
pub(super) fn comment_align(base_line: &str, comment: &str, open: &str) -> String {
    let after = comment.strip_prefix(open).unwrap_or(comment);
    if starts_with_blank_line(after) {
        return base_line.to_owned();
    }
    let (ws, _) = split_while(after, |c| matches!(c, ' ' | '\t'));
    let mut align = String::with_capacity(base_line.len() + open.len() + ws.len());
    align.push_str(base_line);
    align.push_str(open);
    align.push_str(ws);
    align
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
    fn detects_formats_from_media_type() {
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
            let media_type = value.and_then(MediaType::parse);
            assert_eq!(
                BodyFormat::from_media_type(media_type.as_ref()),
                expected,
                "{value:?}"
            );
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
