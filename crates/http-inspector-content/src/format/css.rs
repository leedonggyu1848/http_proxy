//! Lossless CSS token slices with display-only structural whitespace.
use cssparser::{Parser, Token};

use super::{Formatted, Notice, Styled, TokenKind, comment_align};

struct Piece<'a> {
    raw: &'a str,
    token: Token<'a>,
}

fn lex(mut text: &str) -> Vec<Piece<'_>> {
    let mut pieces = Vec::new();
    while !text.is_empty() {
        let mut parser = Parser::new(text);
        let Ok(token) = parser.next_including_whitespace_and_comments().cloned() else {
            break;
        };
        let end = parser.position().byte_index();
        let (raw, rest) = text.split_at(end);
        pieces.push(Piece { raw, token });
        text = rest;
    }
    pieces
}

/// Keeps lexical spelling, including escapes and unterminated tokens. Only
/// whitespace immediately around rule/declaration boundaries is inserted.
/// Never fails: unbalanced or invalid structure is formatted as far as it
/// goes and reported in a notice.
pub(super) fn pretty(text: &str) -> Formatted {
    let pieces = lex(text);
    let mut out = Styled::default();
    let mut stack = Vec::new();
    let mut line_start = true;
    let mut value = false;
    let mut value_align: Option<String> = None;
    let mut invalid = false;
    let mut declaration = is_declaration(&pieces);
    let mut statement = 0;
    for (index, piece) in pieces.iter().enumerate() {
        let token = &piece.token;
        let depth = stack.iter().filter(|c| **c == '}').count();
        let protected = stack.iter().any(|c| *c != '}');
        if line_start && matches!(token, Token::WhiteSpace(_)) {
            continue;
        }
        if !protected
            && !declaration
            && matches!(token, Token::Semicolon | Token::CloseCurlyBracket)
        {
            invalid |= pieces
                .get(statement..index)
                .unwrap_or_default()
                .iter()
                .find(|p| !matches!(p.token, Token::WhiteSpace(_) | Token::Comment(_)))
                .is_some_and(|p| !matches!(p.token, Token::AtKeyword(_)));
        }
        if matches!(token, Token::CloseCurlyBracket) && !protected {
            if stack.pop() != Some('}') {
                invalid = true;
            }
            start_line(&mut out, depth.saturating_sub(1), &mut line_start);
        } else if line_start {
            start_line(&mut out, depth, &mut line_start);
        } else {
            prepare_continuation_indent(&mut out, piece, value, &mut value_align, depth);
        }
        emit(
            &mut out,
            piece,
            value,
            declaration,
            value_align.as_deref(),
            depth,
        );
        invalid |= invalid_token(piece);
        match token {
            Token::Function(_) | Token::ParenthesisBlock => stack.push(')'),
            Token::SquareBracketBlock => stack.push(']'),
            Token::CloseParenthesis => invalid |= stack.pop() != Some(')'),
            Token::CloseSquareBracket => invalid |= stack.pop() != Some(']'),
            Token::CurlyBracketBlock if protected || (value && declaration) => stack.push('v'),
            Token::CloseCurlyBracket if protected => invalid |= stack.pop() != Some('v'),
            Token::Colon if !protected && declaration => {
                value = true;
                value_align = None;
            }
            Token::CurlyBracketBlock | Token::Semicolon | Token::CloseCurlyBracket
                if !protected =>
            {
                if matches!(token, Token::CurlyBracketBlock) {
                    stack.push('}');
                }
                value = false;
                value_align = None;
                statement = index + 1;
                declaration = is_declaration(pieces.get(statement..).unwrap_or_default());
                line_start = true;
            }
            _ => {}
        }
    }
    invalid |= !stack.is_empty();
    Formatted {
        styled: out,
        notices: invalid
            .then(|| Notice::warning("Incomplete or invalid CSS; showing the available prefix. CSS property values are not validated."))
            .into_iter()
            .collect(),
    }
}

fn prepare_continuation_indent(
    out: &mut Styled,
    piece: &Piece<'_>,
    value: bool,
    value_align: &mut Option<String>,
    depth: usize,
) {
    if value && value_align.is_none() {
        if matches!(piece.token, Token::WhiteSpace(_)) {
            if piece.raw.contains('\n') {
                *value_align = Some("  ".repeat(depth + 1));
            }
        } else {
            *value_align = Some(out.visual_current_line());
        }
    }
    if out.text().ends_with('\n') && !piece.raw.starts_with('\n') && !piece.raw.starts_with("\r\n")
    {
        let fallback = "  ".repeat(depth);
        let align = if value {
            value_align.as_deref().unwrap_or(&fallback)
        } else {
            &fallback
        };
        out.indent_line_if_at_start(align);
    }
}

fn emit(
    out: &mut Styled,
    piece: &Piece<'_>,
    value: bool,
    declaration: bool,
    value_align: Option<&str>,
    depth: usize,
) {
    if let Token::Dimension { unit, .. } = &piece.token {
        let split = piece.raw.len().saturating_sub(unit.len());
        if piece.raw.is_char_boundary(split) && piece.raw.ends_with(unit.as_ref()) {
            let (number, unit) = piece.raw.split_at(split);
            out.push(number, TokenKind::Number);
            out.push(unit, TokenKind::Literal);
        } else {
            out.push(piece.raw, TokenKind::Number);
        }
        return;
    }
    if piece.raw.contains('\n') {
        match &piece.token {
            Token::QuotedString(_) => {
                out.push_quoted_multiline(piece.raw, TokenKind::String);
                return;
            }
            Token::Comment(_) => {
                let align = comment_align(&out.visual_current_line(), piece.raw, "/*");
                out.push_multiline(piece.raw, TokenKind::Comment, &align);
                return;
            }
            Token::WhiteSpace(_) => {
                let fallback = "  ".repeat(depth);
                let align = if value {
                    value_align.unwrap_or(&fallback)
                } else {
                    &fallback
                };
                out.push_multiline(piece.raw, TokenKind::Text, align);
                return;
            }
            Token::UnquotedUrl(_) => {
                let align = out.visual_current_line();
                out.push_multiline(piece.raw, TokenKind::String, &align);
                return;
            }
            _ => {}
        }
    }
    out.push(piece.raw, kind(&piece.token, value, declaration));
}

fn invalid_token(piece: &Piece<'_>) -> bool {
    match piece.token {
        Token::BadString(_) | Token::BadUrl(_) => true,
        Token::UnquotedUrl(_) => !piece.raw.ends_with(')'),
        Token::Comment(_) => !piece.raw.ends_with("*/"),
        Token::QuotedString(_) => {
            piece.raw.len() < 2 || piece.raw.chars().next() != piece.raw.chars().last()
        }
        _ => false,
    }
}

// Look ahead once per statement so pseudo-classes stay selector tokens.
fn is_declaration(pieces: &[Piece<'_>]) -> bool {
    if pieces
        .iter()
        .find(|p| !matches!(p.token, Token::WhiteSpace(_) | Token::Comment(_)))
        .is_some_and(|p| matches!(&p.token, Token::Ident(name) if name.starts_with("--")))
    {
        return true;
    }
    let mut colon = false;
    for piece in pieces {
        match piece.token {
            Token::Colon => colon = true,
            Token::CurlyBracketBlock => return false,
            Token::Semicolon | Token::CloseCurlyBracket => return colon,
            _ => {}
        }
    }
    colon
}

fn start_line(out: &mut Styled, depth: usize, pending: &mut bool) {
    if !out.text().is_empty() && !out.text().ends_with('\n') {
        out.push("\n", TokenKind::Text);
    }
    out.push(&"  ".repeat(depth), TokenKind::Text);
    *pending = false;
}

fn kind(token: &Token<'_>, value: bool, declaration: bool) -> TokenKind {
    match token {
        Token::Comment(_) => TokenKind::Comment,
        Token::AtKeyword(_) => TokenKind::Literal,
        Token::QuotedString(_) | Token::UnquotedUrl(_) => TokenKind::String,
        Token::Dimension { .. } | Token::Percentage { .. } | Token::Number { .. } => {
            TokenKind::Number
        }
        Token::Hash(_) | Token::IDHash(_) | Token::Delim('!') if value => TokenKind::Literal,
        Token::Ident(name) if value && name.eq_ignore_ascii_case("important") => TokenKind::Literal,
        Token::Function(name)
            if value && matches!(name.as_ref(), "rgb" | "rgba" | "hsl" | "hsla") =>
        {
            TokenKind::Literal
        }
        Token::Ident(name)
            if value
                && (cssparser::color::parse_named_color(name).is_ok()
                    || name.eq_ignore_ascii_case("transparent")
                    || name.eq_ignore_ascii_case("currentcolor")) =>
        {
            TokenKind::Literal
        }
        Token::Ident(_) | Token::Function(_) if value => TokenKind::AttributeValue,
        Token::Ident(_) if declaration => TokenKind::Key,
        Token::WhiteSpace(_) => TokenKind::Text,
        Token::Ident(_) | Token::IDHash(_) | Token::Hash(_) | Token::Delim('.' | '&') => {
            TokenKind::Tag
        }
        _ => TokenKind::Punctuation,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::format::NoticeLevel;

    #[test]
    fn nested_rules_and_declarations_have_indented_lines() {
        let out = pretty(
            "@media screen{a:hover{color:#fff;width:2px;display:block;&:focus{color: red}}}",
        );
        assert!(
            out.styled
                .text()
                .contains("    color:#fff;\n    width:2px;")
        );
        assert!(out.styled.text().contains("    &:focus{\n      color: red"));
        assert_eq!(out.notices, []);
        for kind in [
            TokenKind::Tag,
            TokenKind::Literal,
            TokenKind::Key,
            TokenKind::Number,
            TokenKind::AttributeValue,
        ] {
            assert!(out.styled.pieces().any(|(_, k)| k == kind));
        }
    }

    #[test]
    fn preserves_strings_urls_comments_and_escapes() {
        let text = r#"a{content:"a;{}\"b";background:url(data:image/svg+xml;a{b});x:url( "a;b" );}/*# sourceMappingURL=a.map */"#;
        let pieces = lex(text);
        assert_eq!(pieces.iter().map(|p| p.raw).collect::<String>(), text);
        let out = pretty(text);
        for protected in [
            r#""a;{}\"b""#,
            "url(data:image/svg+xml;a{b})",
            "url( \"a;b\" )",
            "/*# sourceMappingURL=a.map */",
        ] {
            assert!(out.styled.text().contains(protected), "{protected}");
        }
    }

    #[test]
    fn multiline_comments_and_escaped_identifiers_keep_their_spelling() {
        let text = "/* first\n second */a\\:b{content:\"한글\";color:rebeccapurple!important}";
        let out = pretty(text);
        assert!(out.styled.text().contains("/* first\n second */"));
        assert!(out.styled.text().contains("a\\:b"));
        assert!(
            out.styled
                .pieces()
                .any(|(s, k)| s.contains("rebeccapurple") && k == TokenKind::Literal)
        );
        assert_eq!(out.notices, []);
    }

    #[test]
    fn custom_property_blocks_keep_internal_whitespace() {
        let out = pretty("a{--theme:{a:b;c:d};color:var(--theme)}");
        assert!(out.styled.text().contains("--theme:{a:b;c:d};"));
        assert_eq!(out.notices, []);
    }

    #[test]
    fn partial_and_bad_input_keeps_available_text() {
        for text in [
            "a{color:red",
            "a{content:\"abc",
            "/* unfinished",
            "a{color:red)}",
            "a{color red;}",
        ] {
            let out = pretty(text);
            assert!(
                matches!(out.notices.as_slice(), [notice] if notice.level == NoticeLevel::Warning),
                "{text}"
            );
            assert_ne!(out.styled.text(), "");
        }
    }

    #[test]
    fn visually_indents_multiline_values_strings_and_comments_without_altering_text() {
        let css = "a{background:linear-gradient(\nred,\nblue\n);content:\"first\\\nsecond\\\nthird\";/* c1\nc2 */}";
        let out = pretty(css);
        let text = out.styled.text();
        assert!(text.contains("linear-gradient(\nred,\nblue\n);"), "{text}");
        assert!(text.contains("\"first\\\nsecond\\\nthird\""), "{text}");
        assert!(text.contains("/* c1\nc2 */"), "{text}");

        let indents = out.styled.indents();
        let align_for = |needle: &str| {
            let pos = text.find(needle)?;
            indents
                .iter()
                .find(|indent| indent.offset == pos)
                .map(|indent| indent.align_to.as_str())
        };
        assert_eq!(align_for("red,"), Some("  background:"));
        assert_eq!(align_for("blue"), Some("  background:"));
        assert_eq!(align_for("second\\"), Some("  content:\""));
        assert_eq!(align_for("third\""), Some("  content:\""));
        assert_eq!(align_for("c2 */"), Some("  /* "));
    }
}
