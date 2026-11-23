//! Indented, highlighted XML that keeps text, `CDATA`, and comments as
//! written.

use super::{Document, Element, NodeKind};
use crate::ui::body_format::tag::{Dialect, Spacing, push_tag};
use crate::ui::body_format::{
    Formatted, Notice, Styled, TokenKind, comment_align, split_trailing_blank_line,
    starts_with_blank_line,
};

/// Indents element-only content two spaces per level. Elements that hold
/// text, entity references, or `CDATA`, and elements under
/// `xml:space="preserve"`, keep their content exactly as written. Warns
/// when the document was not read to its end.
pub(in crate::ui::body_format) fn pretty(text: &str) -> Result<Formatted, String> {
    let document = Document::parse(text)?;
    let mut printer = Printer {
        document: &document,
        out: Styled::default(),
    };
    printer.nodes(&document.roots, 0);
    Ok(Formatted {
        styled: printer.out,
        notices: document
            .incomplete_notice()
            .map(Notice::warning)
            .into_iter()
            .collect(),
    })
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
                let trimmed = raw.trim();
                if trimmed.contains('\n') {
                    let align = leaf_align(&self.out.visual_current_line(), kind, trimmed, depth);
                    push_leaf_multiline(&mut self.out, kind, trimmed, &align);
                } else {
                    push_leaf(&mut self.out, kind, trimmed);
                }
            }
        }
    }

    fn element(&mut self, start_tag: &str, element: &Element, depth: usize) {
        self.line(depth);
        push_tag(&mut self.out, start_tag, Dialect::Xml, Spacing::Normalize);
        if element.preserve || has_text(self.document, element) {
            let align = self.verbatim_align(element, depth);
            let last_index = element.children.len().saturating_sub(1);
            for (pos, &child) in element.children.iter().enumerate() {
                let before_close = element.end_tag.is_some() && pos == last_index;
                self.verbatim(child, &align, before_close);
            }
        } else {
            self.nodes(&element.children, depth + 1);
            if element.end_tag.is_some() && !element.children.is_empty() {
                self.line(depth);
            }
        }
        if let Some(end_tag) = &element.end_tag {
            self.out.indent_line_if_at_start(&"  ".repeat(depth));
            push_raw_tag(&mut self.out, self.document.slice(end_tag));
        }
    }

    /// Alignment prefix for continuation lines inside a verbatim element.
    fn verbatim_align(&self, element: &Element, depth: usize) -> String {
        if let [only] = element.children.as_slice()
            && let Some(node) = self.document.node(*only)
            && node.kind == NodeKind::CData
        {
            let raw = self.document.slice(&node.range);
            return leaf_align(&self.out.visual_current_line(), &node.kind, raw, depth);
        }
        let first_is_blank = element
            .children
            .first()
            .and_then(|&child| self.document.node(child))
            .is_some_and(|node| starts_with_blank_line(self.document.slice(&node.range)));
        if first_is_blank {
            "  ".repeat(depth + 1)
        } else {
            self.out.visual_current_line()
        }
    }

    /// Appends a node and its descendants exactly as written while recording
    /// visual continuation indents for multiline content.
    fn verbatim(&mut self, index: usize, align: &str, before_close: bool) {
        let Some(node) = self.document.node(index) else {
            return;
        };
        let raw = self.document.slice(&node.range);
        match &node.kind {
            NodeKind::Element(element) => {
                self.out.indent_line_if_at_start(align);
                push_raw_tag(&mut self.out, raw);
                for &child in &element.children {
                    self.verbatim(child, align, false);
                }
                if let Some(end_tag) = &element.end_tag {
                    self.out.indent_line_if_at_start(align);
                    push_raw_tag(&mut self.out, self.document.slice(end_tag));
                }
            }
            kind => {
                if !raw.is_empty() && !raw.starts_with('\n') && !raw.starts_with("\r\n") {
                    self.out.indent_line_if_at_start(align);
                }
                if before_close && *kind == NodeKind::Text {
                    let (body, tail) = split_trailing_blank_line(raw);
                    push_leaf_multiline(&mut self.out, kind, body, align);
                    self.out.push(tail, TokenKind::Text);
                } else {
                    push_leaf_multiline(&mut self.out, kind, raw, align);
                }
            }
        }
    }

    fn line(&mut self, depth: usize) {
        if !self.out.text().is_empty() {
            self.out.newline(depth);
        }
    }
}

/// Alignment prefix for a multiline non-element node.
fn leaf_align(base_line: &str, kind: &NodeKind, raw: &str, depth: usize) -> String {
    match kind {
        NodeKind::Comment => comment_align(base_line, raw, "<!--"),
        NodeKind::CData => {
            let after = raw.strip_prefix("<![CDATA[").unwrap_or(raw);
            if starts_with_blank_line(after) {
                "  ".repeat(depth + 1)
            } else {
                format!("{base_line}<![CDATA[")
            }
        }
        _ => base_line.to_owned(),
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
    if let Some(token_kind) = leaf_token_kind(kind) {
        out.push(raw, token_kind);
    } else {
        push_raw_tag(out, raw);
    }
}

fn push_leaf_multiline(out: &mut Styled, kind: &NodeKind, raw: &str, align: &str) {
    if let Some(token_kind) = leaf_token_kind(kind) {
        out.push_multiline(raw, token_kind, align);
    } else {
        push_raw_tag(out, raw);
    }
}

const fn leaf_token_kind(kind: &NodeKind) -> Option<TokenKind> {
    match kind {
        NodeKind::Element(_) => None,
        NodeKind::Text => Some(TokenKind::Text),
        NodeKind::Entity => Some(TokenKind::Entity),
        NodeKind::CData => Some(TokenKind::CData),
        NodeKind::Comment => Some(TokenKind::Comment),
        NodeKind::Instruction => Some(TokenKind::Instruction),
    }
}

fn push_raw_tag(out: &mut Styled, tag: &str) {
    push_tag(out, tag, Dialect::Xml, Spacing::AsWritten);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(input: &str) -> Result<String, String> {
        Ok(pretty(input)?.styled.text().to_owned())
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
        let Formatted { styled, notices } = pretty(soap)?;
        assert_eq!(notices, []);
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
        let Formatted { styled, notices } = pretty("<feed><entry><id>1</id></entry><entry><id>2")?;
        assert_eq!(
            styled.text(),
            "<feed>\n  <entry>\n    <id>1</id>\n  </entry>\n  <entry>\n    <id>2"
        );
        assert!(
            matches!(notices.as_slice(), [notice] if notice.text.contains("<id> is not closed"))
        );
        let Formatted { styled, notices } = pretty("<a><b>1</b><c></a><d/>")?;
        assert_eq!(styled.text(), "<a>\n  <b>1</b>\n  <c>");
        assert!(matches!(notices.as_slice(), [notice] if notice.text.contains("incomplete")));
        assert!(pretty("{\"json\": true}").is_err());
        Ok(())
    }

    #[test]
    fn leaves_entities_of_a_dtd_unexpanded() -> Result<(), String> {
        let input = "<!DOCTYPE r [<!ENTITY e SYSTEM \"file:///etc/passwd\">]><r>&e;</r>";
        let Formatted { styled, notices } = pretty(input)?;
        assert_eq!(notices, []);
        assert_eq!(
            styled.text(),
            "<!DOCTYPE r [<!ENTITY e SYSTEM \"file:///etc/passwd\">]>\n<r>&e;</r>"
        );
        assert_eq!(kinds(&styled, TokenKind::Entity), ["&e;"]);
        Ok(())
    }

    #[test]
    fn aligns_multiline_element_text_attributes_cdata_and_comments_without_padding_text()
    -> Result<(), String> {
        let xml = concat!(
            "<root><outer>",
            "<note>first\nsecond\nthird</note>",
            "<item attr=\"a1 &amp;\na2\na3\" second=\"b1\nb2\"/>",
            "<data><![CDATA[c1\nc2\nc3]]></data>",
            "<!-- m1\nm2\nm3 -->",
            "<next>ok</next>",
            "</outer></root>"
        );
        let styled = pretty(xml)?.styled;
        // Source newlines and spaces inside values stay untouched in styled.text().
        assert!(
            styled
                .text()
                .contains("    <note>first\nsecond\nthird</note>")
        );
        assert!(
            styled
                .text()
                .contains("    <item attr=\"a1 &amp;\na2\na3\" second=\"b1\nb2\"/>")
        );
        assert!(
            styled
                .text()
                .contains("    <data><![CDATA[c1\nc2\nc3]]></data>")
        );
        assert!(styled.text().contains("    <!-- m1\nm2\nm3 -->"));

        let align_for = |needle: &str| -> Option<&str> {
            let offset = styled.text().find(needle)?;
            styled
                .indents()
                .iter()
                .find(|indent| indent.offset == offset)
                .map(|indent| indent.align_to.as_str())
        };
        assert_eq!(align_for("second\n"), Some("    <note>"));
        assert_eq!(align_for("third</note>"), Some("    <note>"));
        assert_eq!(align_for("a2\n"), Some("    <item attr=\""));
        assert_eq!(align_for("a3\""), Some("    <item attr=\""));
        assert_eq!(align_for("b2\""), Some("    <item attr=\"a3\" second=\""));
        assert_eq!(align_for("c2\n"), Some("    <data><![CDATA["));
        assert_eq!(align_for("c3]]>"), Some("    <data><![CDATA["));
        assert_eq!(align_for("m2\n"), Some("    <!-- "));
        assert_eq!(align_for("m3 -->"), Some("    <!-- "));
        Ok(())
    }

    #[test]
    fn indents_multiline_element_starting_with_newline_and_aligns_closing_tag() -> Result<(), String>
    {
        let xml = "<root><outer><note>\nline1\nline2\nline3\n</note><next>ok</next></outer></root>";
        let styled = pretty(xml)?.styled;
        assert!(
            styled
                .text()
                .contains("    <note>\nline1\nline2\nline3\n</note>")
        );
        let align_for = |needle: &str| -> Option<&str> {
            let offset = styled.text().find(needle)?;
            styled
                .indents()
                .iter()
                .find(|indent| indent.offset == offset)
                .map(|indent| indent.align_to.as_str())
        };
        assert_eq!(align_for("line1"), Some("      "));
        assert_eq!(align_for("line2"), Some("      "));
        assert_eq!(align_for("line3"), Some("      "));
        assert_eq!(align_for("</note>"), Some("    "));
        Ok(())
    }
}
