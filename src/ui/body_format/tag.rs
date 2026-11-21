//! Splits an HTML or XML tag into highlighted tokens, shared by the HTML
//! pretty printer and the XML pretty and tree views.

use super::{Styled, TokenKind, split_while};

/// The markup language of a tag, which decides the few rules that differ
/// between HTML and XML.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Dialect {
    /// Unquoted values run to the next space or `>`, values are one token,
    /// and a normalized empty-element tag ends in ` />`.
    Html,
    /// `prefix:` of names is marked as a namespace, unquoted values stop at
    /// the next name boundary, and entity references in values are marked.
    Xml,
}

/// How whitespace inside a tag is shown. Attribute values are never changed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Spacing {
    /// Whitespace between attributes becomes one space and is dropped around
    /// `=` and before the tag end, so a tag written over several lines stays
    /// on one.
    Normalize,
    /// Whitespace is kept exactly as written.
    AsWritten,
}

/// Appends an opening, closing, or empty-element tag with its names,
/// attribute names, and attribute values as separate tokens. A stray
/// character such as `=` without a name or a lone quote is kept as written.
pub(super) fn push_tag(out: &mut Styled, tag: &str, dialect: Dialect, spacing: Spacing) {
    let normalize = spacing == Spacing::Normalize;
    let open = if tag.starts_with("</") { "</" } else { "<" };
    let rest = tag.get(open.len()..).unwrap_or_default();
    let (name, mut rest) = split_while(rest, is_name_char);
    out.push(open, TokenKind::Tag);
    push_name(out, name, TokenKind::Tag, dialect);
    loop {
        let (space, after) = split_while(rest, char::is_whitespace);
        rest = after;
        let end = rest.is_empty() || rest.starts_with('>') || rest.starts_with("/>");
        if normalize {
            if !end && !space.is_empty() {
                out.push(" ", TokenKind::Text);
            }
        } else {
            out.push(space, TokenKind::Text);
        }
        if end {
            break;
        }
        let (attribute, after) = split_while(rest, is_name_char);
        if attribute.is_empty() {
            let (stray, after) = rest.split_at(rest.chars().next().map_or(0, char::len_utf8));
            out.push(stray, TokenKind::Punctuation);
            rest = after;
            continue;
        }
        push_name(out, attribute, TokenKind::AttributeName, dialect);
        rest = after;
        let (space, after) = split_while(rest, char::is_whitespace);
        if let Some(value) = after.strip_prefix('=') {
            out.push(if normalize { "" } else { space }, TokenKind::Text);
            out.push("=", TokenKind::Punctuation);
            let (space, value) = split_while(value, char::is_whitespace);
            out.push(if normalize { "" } else { space }, TokenKind::Text);
            let (value, after) = split_value(value, dialect);
            push_value(out, value, dialect);
            rest = after;
        }
    }
    if dialect == Dialect::Html && normalize && open == "<" && rest.starts_with("/>") {
        out.push(" ", TokenKind::Text);
    }
    out.push(rest, TokenKind::Tag);
}

const fn is_name_char(c: char) -> bool {
    !c.is_whitespace() && !matches!(c, '/' | '>' | '=' | '"' | '\'')
}

/// Appends a name; in XML, `prefix:local` has its prefix and colon marked
/// as a namespace.
fn push_name(out: &mut Styled, name: &str, kind: TokenKind, dialect: Dialect) {
    match name.split_once(':') {
        Some((prefix, local)) if dialect == Dialect::Xml && !prefix.is_empty() => {
            out.push(prefix, TokenKind::Namespace);
            out.push(":", TokenKind::Punctuation);
            out.push(local, kind);
        }
        _ => out.push(name, kind),
    }
}

/// Splits a value, including its quotes, from the rest of a tag. An
/// unterminated quoted value runs to the end.
fn split_value(text: &str, dialect: Dialect) -> (&str, &str) {
    let Some(quote) = text.chars().next().filter(|c| matches!(c, '"' | '\'')) else {
        return match dialect {
            Dialect::Html => split_while(text, |c| !c.is_whitespace() && c != '>'),
            Dialect::Xml => split_while(text, is_name_char),
        };
    };
    let end = text
        .get(1..)
        .and_then(|inner| inner.find(quote))
        .map_or(text.len(), |index| index + 2);
    text.split_at(end)
}

/// Appends an attribute value; in XML, entity references are marked.
fn push_value(out: &mut Styled, value: &str, dialect: Dialect) {
    if dialect == Dialect::Html {
        out.push(value, TokenKind::AttributeValue);
        return;
    }
    let mut rest = value;
    while let Some(start) = rest.find('&') {
        let (before, reference) = rest.split_at(start);
        out.push(before, TokenKind::AttributeValue);
        let end = reference
            .find(';')
            .filter(|&end| {
                !reference
                    .get(1..end)
                    .unwrap_or_default()
                    .contains(['"', '\''])
            })
            .map_or(1, |end| end + 1);
        let (reference, after) = reference.split_at(end);
        out.push(
            reference,
            if end > 1 {
                TokenKind::Entity
            } else {
                TokenKind::AttributeValue
            },
        );
        rest = after;
    }
    out.push(rest, TokenKind::AttributeValue);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tokens(tag: &str, dialect: Dialect, spacing: Spacing) -> Vec<(String, TokenKind)> {
        let mut out = Styled::default();
        push_tag(&mut out, tag, dialect, spacing);
        out.pieces()
            .filter(|(text, _)| !text.is_empty())
            .map(|(text, kind)| (text.to_owned(), kind))
            .collect()
    }

    fn text(tag: &str, dialect: Dialect, spacing: Spacing) -> String {
        tokens(tag, dialect, spacing)
            .into_iter()
            .map(|(text, _)| text)
            .collect()
    }

    #[test]
    fn marks_namespaces_and_entities_only_in_xml() {
        let tag = "<x:a x:b=\"1 &amp; 2\">";
        let html = tokens(tag, Dialect::Html, Spacing::Normalize);
        let xml = tokens(tag, Dialect::Xml, Spacing::Normalize);
        assert!(html.iter().all(|(_, kind)| *kind != TokenKind::Namespace));
        assert!(html.contains(&("<x:a".to_owned(), TokenKind::Tag)));
        assert!(html.contains(&("\"1 &amp; 2\"".to_owned(), TokenKind::AttributeValue)));
        assert!(xml.contains(&("x".to_owned(), TokenKind::Namespace)));
        assert!(xml.contains(&("&amp;".to_owned(), TokenKind::Entity)));
    }

    #[test]
    fn ends_unquoted_values_by_dialect() {
        let value = |dialect| {
            tokens("<a href=/x/y>", dialect, Spacing::Normalize)
                .into_iter()
                .find(|(_, kind)| *kind == TokenKind::AttributeValue)
                .map(|(text, _)| text)
        };
        assert_eq!(value(Dialect::Html).as_deref(), Some("/x/y"));
        assert_eq!(value(Dialect::Xml), None);
    }

    #[test]
    fn spaces_empty_element_ends_only_in_normalized_html() {
        assert_eq!(text("<br/>", Dialect::Html, Spacing::Normalize), "<br />");
        assert_eq!(text("<br  />", Dialect::Html, Spacing::Normalize), "<br />");
        assert_eq!(text("<br/>", Dialect::Xml, Spacing::Normalize), "<br/>");
        assert_eq!(text("<br  />", Dialect::Xml, Spacing::AsWritten), "<br  />");
    }

    #[test]
    fn keeps_stray_characters_as_written() {
        for dialect in [Dialect::Html, Dialect::Xml] {
            let tokens = tokens("<p =x \"y\">", dialect, Spacing::Normalize);
            let text: String = tokens.iter().map(|(text, _)| text.as_str()).collect();
            assert_eq!(text, "<p =x \"y\">");
            for stray in ["=", "\""] {
                assert!(tokens.contains(&(stray.to_owned(), TokenKind::Punctuation)));
            }
            assert!(tokens.contains(&("x".to_owned(), TokenKind::AttributeName)));
        }
    }
}
