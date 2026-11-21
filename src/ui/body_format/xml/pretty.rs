//! Indented, highlighted XML that keeps text, `CDATA`, and comments as
//! written.

use super::{Document, Element, NodeKind};
use crate::ui::body_format::tag::{Dialect, Spacing, push_tag};
use crate::ui::body_format::{Styled, TokenKind};

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
        push_tag(&mut self.out, start_tag, Dialect::Xml, Spacing::Normalize);
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
            push_raw_tag(&mut self.out, self.document.slice(end_tag));
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
    push_tag(out, tag, Dialect::Xml, Spacing::AsWritten);
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
