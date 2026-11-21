//! Token-based HTML and XML pretty printer that tolerates truncated and
//! unbalanced markup.

use super::tag::{Dialect, Spacing, push_tag};
use super::{Styled, TokenKind, split_while};

/// HTML elements that never have a closing tag.
const VOID_ELEMENTS: [&str; 14] = [
    "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "param", "source",
    "track", "wbr",
];
/// HTML elements whose content is shown exactly as written: raw text
/// rather than markup (`script`, `style`), or text whose whitespace is
/// meaningful (`pre`, `textarea`).
const VERBATIM_ELEMENTS: [&str; 4] = ["script", "style", "pre", "textarea"];
/// Longest text kept on the same line as its enclosing tags.
const INLINE_TEXT: usize = 80;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Node<'a> {
    /// `<!DOCTYPE …>`, `<?xml …?>`, and other declarations.
    Declaration(&'a str),
    Comment(&'a str),
    Open {
        tag: &'a str,
        name: &'a str,
        has_content: bool,
    },
    Close {
        tag: &'a str,
        name: &'a str,
    },
    Text(&'a str),
    /// Content of a [`VERBATIM_ELEMENTS`] element, never trimmed or re-indented.
    Verbatim(&'a str),
}

/// Puts each tag, comment, and text run on its own line and indents element
/// content. Short text stays inline between its opening and closing tags.
pub(super) fn pretty(text: &str) -> Result<Styled, String> {
    let nodes = lex(text);
    if nodes.iter().all(|node| matches!(node, Node::Text(_))) {
        return Err("no tags found".to_owned());
    }
    let mut out = Styled::default();
    let mut depth = 0_usize;
    let mut index = 0;
    while let Some(node) = nodes.get(index).copied() {
        index += 1;
        match node {
            Node::Declaration(declaration) => {
                line(&mut out, depth);
                out.push(declaration, TokenKind::Tag);
            }
            Node::Comment(comment) => {
                line(&mut out, depth);
                out.push(comment, TokenKind::Comment);
            }
            // Only reached when the closing tag was cut off.
            Node::Verbatim(text) => out.push(text, TokenKind::Text),
            Node::Text(text) => {
                for text_line in text.lines().map(str::trim).filter(|line| !line.is_empty()) {
                    line(&mut out, depth);
                    out.push(text_line, TokenKind::Text);
                }
            }
            Node::Open {
                tag,
                name,
                has_content,
            } => {
                line(&mut out, depth);
                push_tag(&mut out, tag, Dialect::Html, Spacing::Normalize);
                if has_content {
                    let inline = inline_content(&nodes, index, name);
                    if let Some((consumed, text, close)) = inline {
                        out.push(text, TokenKind::Text);
                        push_tag(&mut out, close, Dialect::Html, Spacing::Normalize);
                        index += consumed;
                    } else {
                        depth += 1;
                    }
                }
            }
            Node::Close { tag, .. } => {
                depth = depth.saturating_sub(1);
                line(&mut out, depth);
                push_tag(&mut out, tag, Dialect::Html, Spacing::Normalize);
            }
        }
    }
    Ok(out)
}

/// Finds `[short text] </name>` or `verbatim content </name>` right after an
/// opening tag and returns how many nodes it spans, the text to show between
/// the tags, and the closing tag.
fn inline_content<'a>(
    nodes: &[Node<'a>],
    index: usize,
    name: &str,
) -> Option<(usize, &'a str, &'a str)> {
    let closes = |node: Option<&Node<'a>>| match node {
        Some(Node::Close { tag, name: close }) if close.eq_ignore_ascii_case(name) => Some(*tag),
        _ => None,
    };
    if let Some(close) = closes(nodes.get(index)) {
        return Some((1, "", close));
    }
    if let Some(Node::Verbatim(text)) = nodes.get(index) {
        return Some((2, text, closes(nodes.get(index + 1))?));
    }
    let Some(Node::Text(text)) = nodes.get(index) else {
        return None;
    };
    let text = text.trim();
    let close = closes(nodes.get(index + 1))?;
    (!text.contains('\n') && text.chars().count() <= INLINE_TEXT).then_some((2, text, close))
}

fn line(out: &mut Styled, depth: usize) {
    if !out.text().is_empty() {
        out.newline(depth);
    }
}

fn lex(text: &str) -> Vec<Node<'_>> {
    let mut nodes = Vec::new();
    let mut rest = text;
    while !rest.is_empty() {
        let (node, after) = next_node(rest);
        nodes.push(node);
        rest = after;
        if let Node::Open {
            name,
            has_content: true,
            ..
        } = node
            && VERBATIM_ELEMENTS
                .iter()
                .any(|verbatim| verbatim.eq_ignore_ascii_case(name))
        {
            let end = find_ignore_case(rest, &format!("</{name}")).unwrap_or(rest.len());
            let (content, after) = rest.split_at(end);
            if !content.is_empty() {
                nodes.push(Node::Verbatim(content));
            }
            rest = after;
        }
    }
    nodes
}

fn next_node(rest: &str) -> (Node<'_>, &str) {
    if rest.starts_with("<!--") {
        let (comment, after) = split_after(rest, "-->");
        return (Node::Comment(comment), after);
    }
    if rest.starts_with("<![CDATA[") {
        let (cdata, after) = split_after(rest, "]]>");
        return (Node::Text(cdata), after);
    }
    if rest.starts_with("<!") || rest.starts_with("<?") {
        let (declaration, after) = split_after(rest, ">");
        return (Node::Declaration(declaration), after);
    }
    let mut chars = rest.chars();
    let opening = chars.next() == Some('<');
    let closing = opening && rest.starts_with("</");
    let name_start = chars
        .nth(usize::from(closing))
        .is_some_and(char::is_alphabetic);
    if !opening || !name_start {
        // Text runs to the next `<`, past a `<` that starts no tag.
        let end = rest
            .char_indices()
            .skip(1)
            .find(|(_, c)| *c == '<')
            .map_or(rest.len(), |(index, _)| index);
        let (text, after) = rest.split_at(end);
        return (Node::Text(text), after);
    }
    let (tag, after) = rest.split_at(tag_len(rest));
    let name = tag_name(tag);
    let node = if closing {
        Node::Close { tag, name }
    } else {
        Node::Open {
            tag,
            name,
            has_content: !tag.ends_with("/>")
                && !VOID_ELEMENTS
                    .iter()
                    .any(|void| void.eq_ignore_ascii_case(name)),
        }
    };
    (node, after)
}

/// Splits after the first `end`, or at the end of `text` when it is missing.
fn split_after<'a>(text: &'a str, end: &str) -> (&'a str, &'a str) {
    text.split_at(text.find(end).map_or(text.len(), |index| index + end.len()))
}

fn find_ignore_case(text: &str, needle: &str) -> Option<usize> {
    text.char_indices().map(|(index, _)| index).find(|&index| {
        text.get(index..index + needle.len())
            .is_some_and(|candidate| candidate.eq_ignore_ascii_case(needle))
    })
}

/// Length of the tag at the start of `text` up to its `>`, skipping quoted
/// attribute values, or the rest of `text` when the tag is unterminated.
fn tag_len(text: &str) -> usize {
    let mut quote = None;
    for (index, c) in text.char_indices() {
        match (quote, c) {
            (None, '"' | '\'') => quote = Some(c),
            (Some(open), _) if open == c => quote = None,
            (None, '>') => return index + 1,
            _ => {}
        }
    }
    text.len()
}

fn tag_name(tag: &str) -> &str {
    let inner = tag.trim_start_matches('<').trim_start_matches('/');
    split_while(inner, |c| !c.is_whitespace() && !matches!(c, '/' | '>')).0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(styled: &Styled, kind: TokenKind) -> Vec<&str> {
        styled
            .pieces()
            .filter(|(_, piece_kind)| *piece_kind == kind)
            .map(|(text, _)| text)
            .collect()
    }

    #[test]
    fn indents_minified_html() -> Result<(), String> {
        let styled = pretty(concat!(
            "<!DOCTYPE html><html lang=en><head><meta charset=\"utf-8\">",
            "<title>Test page</title><!-- note --></head><body>",
            "<p class='a b' hidden>Hi <b>there</b></p><br/><div></div></body></html>"
        ))?;
        assert_eq!(
            styled.text(),
            concat!(
                "<!DOCTYPE html>\n<html lang=en>\n  <head>\n    <meta charset=\"utf-8\">\n",
                "    <title>Test page</title>\n    <!-- note -->\n  </head>\n  <body>\n",
                "    <p class='a b' hidden>\n      Hi\n      <b>there</b>\n    </p>\n",
                "    <br />\n    <div></div>\n  </body>\n</html>"
            )
        );
        assert_eq!(
            kinds(&styled, TokenKind::AttributeName),
            ["lang", "charset", "class", "hidden"]
        );
        assert_eq!(
            kinds(&styled, TokenKind::AttributeValue),
            ["en", "\"utf-8\"", "'a b'"]
        );
        assert_eq!(kinds(&styled, TokenKind::Comment), ["<!-- note -->"]);
        Ok(())
    }

    #[test]
    fn keeps_script_content_and_xml_declarations() -> Result<(), String> {
        let styled =
            pretty("<?xml version=\"1.0\"?><feed><script>if (a<b) {}</script><e/></feed>")?;
        assert_eq!(
            styled.text(),
            "<?xml version=\"1.0\"?>\n<feed>\n  <script>if (a<b) {}</script>\n  <e />\n</feed>"
        );
        Ok(())
    }

    #[test]
    fn keeps_whitespace_of_pre_textarea_and_script_content() -> Result<(), String> {
        let pre = "\n  first\n\n    indented <b>bold</b>\n";
        let textarea = "  line 1\n\n  line 2 ";
        let script = "\n  if (a) {\n    b();\n  }\n";
        let styled = pretty(&format!(
            "<body><PRE class=x>{pre}</PRE><textarea>{textarea}</textarea><script>{script}</script></body>"
        ))?;
        assert_eq!(
            styled.text(),
            format!(
                "<body>\n  <PRE class=x>{pre}</PRE>\n  <textarea>{textarea}</textarea>\n  <script>{script}</script>\n</body>"
            )
        );
        assert_eq!(
            pretty(&format!("<pre>{pre}"))?.text(),
            format!("<pre>{pre}"),
            "content cut before the closing tag is kept as well"
        );
        Ok(())
    }

    #[test]
    fn formats_truncated_and_unbalanced_markup() -> Result<(), String> {
        assert_eq!(pretty("<div><a href=\"/x")?.text(), "<div>\n  <a href=\"/x");
        assert_eq!(pretty("</p></p><!-- cut")?.text(), "</p>\n</p>\n<!-- cut");
        Ok(())
    }

    #[test]
    fn keeps_stray_characters_in_tags_as_written() -> Result<(), String> {
        let styled = pretty("<p =x a=\"1\"b=2 'c'>t</p>")?;
        assert_eq!(styled.text(), "<p =x a=\"1\"b=2 'c'>t</p>");
        assert_eq!(
            kinds(&styled, TokenKind::AttributeName),
            ["x", "a", "b", "c"]
        );
        assert_eq!(kinds(&styled, TokenKind::AttributeValue), ["\"1\"", "2"]);
        assert_eq!(
            kinds(&styled, TokenKind::Punctuation),
            ["=", "=", "=", "'", "'"]
        );
        Ok(())
    }

    #[test]
    fn rejects_text_without_tags() {
        assert!(pretty("plain text, 1 < 2").is_err());
    }
}
