//! Indented, highlighted XML that keeps text, `CDATA`, and comments as
//! written.

use super::{Document, Element, NodeKind};
use crate::ui::body_format::{Styled, TokenKind, split_while};

/// Indents element-only content two spaces per level. Elements that hold
/// text, entity references, or `CDATA`, and elements under
/// `xml:space="preserve"`, keep their content exactly as written. Returns
/// the text and a notice when the document was not read to its end.
pub(in crate::ui::body_format) fn pretty(text: &str) -> Result<(Styled, Option<String>), String> {
    let document = Document::parse(text)?;
    let mut printer = Printer {
        document: &document,
        out: Styled::default(),
    };
    printer.nodes(&document.roots, 0);
    Ok((printer.out, document.incomplete_notice()))
}

struct Printer<'d, 'a> {
    document: &'d Document<'a>,
    out: Styled,
}

impl Printer<'_, '_> {
    fn nodes(&mut self, indexes: &[usize], depth: usize) {
        for &index in indexes {
            self.node(index, depth);
        }
    }

    fn node(&mut self, index: usize, depth: usize) {
        let Some(node) = self.document.node(index) else {
            return;
        };
        let raw = self.document.slice(&node.range);
        match &node.kind {
            NodeKind::Element(element) => self.element(raw, element, depth),
            NodeKind::Text | NodeKind::Entity if raw.trim().is_empty() => {}
            kind => {
                self.line(depth);
                push_leaf(&mut self.out, kind, raw.trim());
            }
        }
    }

    fn element(&mut self, start_tag: &str, element: &Element, depth: usize) {
        self.line(depth);
        push_tag(&mut self.out, start_tag, true);
        if element.preserve || has_text(self.document, element) {
            for &child in &element.children {
                self.verbatim(child);
            }
        } else {
            self.nodes(&element.children, depth + 1);
            if element.end_tag.is_some() && !element.children.is_empty() {
                self.line(depth);
            }
        }
        if let Some(end_tag) = &element.end_tag {
            push_tag(&mut self.out, self.document.slice(end_tag), false);
        }
    }

    /// Appends a node and its descendants exactly as written.
    fn verbatim(&mut self, index: usize) {
        let Some(node) = self.document.node(index) else {
            return;
        };
        let raw = self.document.slice(&node.range);
        match &node.kind {
            NodeKind::Element(element) => {
                push_raw_tag(&mut self.out, raw);
                for &child in &element.children {
                    self.verbatim(child);
                }
                if let Some(end_tag) = &element.end_tag {
                    push_raw_tag(&mut self.out, self.document.slice(end_tag));
                }
            }
            kind => push_leaf(&mut self.out, kind, raw),
        }
    }

    fn line(&mut self, depth: usize) {
        if !self.out.text().is_empty() {
            self.out.newline(depth);
        }
    }
}

/// Whether an element has character content, which makes its whitespace
/// part of the content.
fn has_text(document: &Document<'_>, element: &Element) -> bool {
    element.children.iter().any(|&child| {
        document.node(child).is_some_and(|node| match node.kind {
            NodeKind::Text => !document.slice(&node.range).trim().is_empty(),
            NodeKind::Entity | NodeKind::CData => true,
            _ => false,
        })
    })
}

/// Appends a non-element node with its token kind.
pub(super) fn push_leaf(out: &mut Styled, kind: &NodeKind, raw: &str) {
    let kind = match kind {
        NodeKind::Element(_) => {
            push_raw_tag(out, raw);
            return;
        }
        NodeKind::Text => TokenKind::Text,
        NodeKind::Entity => TokenKind::Entity,
        NodeKind::CData => TokenKind::CData,
        NodeKind::Comment => TokenKind::Comment,
        NodeKind::Instruction => TokenKind::Instruction,
    };
    out.push(raw, kind);
}

fn push_raw_tag(out: &mut Styled, tag: &str) {
    push_tag(out, tag, false);
}

/// Appends an opening, closing, or empty-element tag with its namespace
/// prefixes, names, and attribute values as separate tokens. With
/// `normalize`, whitespace between attributes becomes one space so a tag
/// written over several lines stays on one; values are never changed.
pub(super) fn push_tag(out: &mut Styled, tag: &str, normalize: bool) {
    let open = if tag.starts_with("</") { "</" } else { "<" };
    let rest = tag.get(open.len()..).unwrap_or_default();
    let (name, mut rest) = split_while(rest, is_name_char);
    out.push(open, TokenKind::Tag);
    push_name(out, name, TokenKind::Tag);
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
            // A stray character such as `=` or a quote: keep it as written.
            let (stray, after) = rest.split_at(rest.chars().next().map_or(0, char::len_utf8));
            out.push(stray, TokenKind::Punctuation);
            rest = after;
            continue;
        }
        push_name(out, attribute, TokenKind::AttributeName);
        rest = after;
        let (space, after) = split_while(rest, char::is_whitespace);
        if let Some(value) = after.strip_prefix('=') {
            out.push(if normalize { "" } else { space }, TokenKind::Text);
            out.push("=", TokenKind::Punctuation);
            let (space, value) = split_while(value, char::is_whitespace);
            out.push(if normalize { "" } else { space }, TokenKind::Text);
            let (value, after) = split_quoted(value);
            push_value(out, value);
            rest = after;
        }
    }
    out.push(rest, TokenKind::Tag);
}

const fn is_name_char(c: char) -> bool {
    !c.is_whitespace() && !matches!(c, '/' | '>' | '=' | '"' | '\'')
}

/// Appends `prefix:local` with the prefix and colon marked as a namespace.
fn push_name(out: &mut Styled, name: &str, kind: TokenKind) {
    match name.split_once(':') {
        Some((prefix, local)) if !prefix.is_empty() => {
            out.push(prefix, TokenKind::Namespace);
            out.push(":", TokenKind::Punctuation);
            out.push(local, kind);
        }
        _ => out.push(name, kind),
    }
}

/// Splits a quoted value, including its quotes, from the rest of a tag. An
/// unterminated value runs to the end; an unquoted one to the next space.
fn split_quoted(text: &str) -> (&str, &str) {
    let Some(quote) = text.chars().next().filter(|c| matches!(c, '"' | '\'')) else {
        return split_while(text, is_name_char);
    };
    let end = text
        .get(1..)
        .and_then(|inner| inner.find(quote))
        .map_or(text.len(), |index| index + 2);
    text.split_at(end)
}

/// Appends an attribute value with its entity references marked.
fn push_value(out: &mut Styled, value: &str) {
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

    fn text(input: &str) -> Result<String, String> {
        Ok(pretty(input)?.0.text().to_owned())
    }

    fn kinds(styled: &Styled, kind: TokenKind) -> Vec<&str> {
        styled
            .pieces()
            .filter(|(_, piece_kind)| *piece_kind == kind)
            .map(|(text, _)| text)
            .collect()
    }

    #[test]
    fn indents_one_line_rss() -> Result<(), String> {
        let rss = concat!(
            "<?xml version=\"1.0\"?><rss version=\"2.0\"><channel><title>News</title>",
            "<!-- latest --><item><title>A &amp; B</title><guid isPermaLink=\"false\">1</guid>",
            "</item><item/></channel></rss>"
        );
        assert_eq!(
            text(rss)?,
            concat!(
                "<?xml version=\"1.0\"?>\n",
                "<rss version=\"2.0\">\n",
                "  <channel>\n",
                "    <title>News</title>\n",
                "    <!-- latest -->\n",
                "    <item>\n",
                "      <title>A &amp; B</title>\n",
                "      <guid isPermaLink=\"false\">1</guid>\n",
                "    </item>\n",
                "    <item/>\n",
                "  </channel>\n",
                "</rss>"
            )
        );
        Ok(())
    }

    #[test]
    fn keeps_text_cdata_comments_and_preserved_space() -> Result<(), String> {
        let input = concat!(
            "<doc><p>  two  spaces <b>bold</b> tail </p>",
            "<code xml:space=\"preserve\">\n  <line> x </line>\n</code>",
            "<!--  keep\n   this  --><s><![CDATA[ <not a tag> ]]></s></doc>"
        );
        assert_eq!(
            text(input)?,
            concat!(
                "<doc>\n",
                "  <p>  two  spaces <b>bold</b> tail </p>\n",
                "  <code xml:space=\"preserve\">\n  <line> x </line>\n</code>\n",
                "  <!--  keep\n   this  -->\n",
                "  <s><![CDATA[ <not a tag> ]]></s>\n",
                "</doc>"
            )
        );
        Ok(())
    }

    #[test]
    fn classifies_xml_tokens() -> Result<(), String> {
        let soap = concat!(
            "<?xml version=\"1.0\"?><soap:Envelope xmlns:soap=\"urn:s\" a=\"x &lt; y\">",
            "<soap:Body><!--c--><m>&#65;<![CDATA[d]]></m></soap:Body></soap:Envelope>"
        );
        let (styled, notice) = pretty(soap)?;
        assert_eq!(notice, None);
        assert_eq!(
            kinds(&styled, TokenKind::Instruction),
            ["<?xml version=\"1.0\"?>"]
        );
        assert_eq!(
            kinds(&styled, TokenKind::Namespace),
            ["soap", "xmlns", "soap", "soap", "soap"]
        );
        assert_eq!(
            kinds(&styled, TokenKind::Tag),
            [
                "<",
                "Envelope",
                ">",
                "<",
                "Body>",
                "<m>",
                "</m>",
                "</",
                "Body>",
                "</",
                "Envelope>"
            ]
        );
        assert_eq!(kinds(&styled, TokenKind::AttributeName), ["soap", "a"]);
        assert_eq!(
            kinds(&styled, TokenKind::AttributeValue),
            ["\"urn:s\"", "\"x ", " y\""]
        );
        assert_eq!(kinds(&styled, TokenKind::Entity), ["&lt;", "&#65;"]);
        assert_eq!(kinds(&styled, TokenKind::Comment), ["<!--c-->"]);
        assert_eq!(kinds(&styled, TokenKind::CData), ["<![CDATA[d]]>"]);
        Ok(())
    }

    #[test]
    fn normalizes_space_inside_tags_only() -> Result<(), String> {
        assert_eq!(
            text("<a\n   x = \"1  2\"\n   y='3'  >t</a  >")?,
            "<a x=\"1  2\" y='3'>t</a  >"
        );
        Ok(())
    }

    #[test]
    fn shows_truncated_and_malformed_xml_as_far_as_read() -> Result<(), String> {
        let (styled, notice) = pretty("<feed><entry><id>1</id></entry><entry><id>2")?;
        assert_eq!(
            styled.text(),
            "<feed>\n  <entry>\n    <id>1</id>\n  </entry>\n  <entry>\n    <id>2"
        );
        assert!(notice.is_some_and(|notice| notice.contains("<id> is not closed")));
        let (styled, notice) = pretty("<a><b>1</b><c></a><d/>")?;
        assert_eq!(styled.text(), "<a>\n  <b>1</b>\n  <c>");
        assert!(notice.is_some_and(|notice| notice.contains("incomplete")));
        assert!(pretty("{\"json\": true}").is_err());
        Ok(())
    }

    #[test]
    fn leaves_entities_of_a_dtd_unexpanded() -> Result<(), String> {
        let input = "<!DOCTYPE r [<!ENTITY e SYSTEM \"file:///etc/passwd\">]><r>&e;</r>";
        let (styled, notice) = pretty(input)?;
        assert_eq!(notice, None);
        assert_eq!(
            styled.text(),
            "<!DOCTYPE r [<!ENTITY e SYSTEM \"file:///etc/passwd\">]>\n<r>&e;</r>"
        );
        assert_eq!(kinds(&styled, TokenKind::Entity), ["&e;"]);
        Ok(())
    }
}
