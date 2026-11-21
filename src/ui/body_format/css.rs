//! Lossless CSS token slices with display-only structural whitespace.
use cssparser::{Parser, Token};

use super::{Formatted, Notice, Styled, TokenKind};

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
#[expect(
    clippy::unnecessary_wraps,
    reason = "every pretty printer shares one signature"
)]
pub(super) fn pretty(text: &str) -> Result<Formatted, String> {
    let pieces = lex(text);
    let mut out = Styled::default();
    let mut stack = Vec::new();
    let mut line_start = true;
    let mut value = false;
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
        }
        emit(&mut out, piece, value, declaration);
        invalid |= invalid_token(piece);
        match token {
            Token::Function(_) | Token::ParenthesisBlock => stack.push(')'),
            Token::SquareBracketBlock => stack.push(']'),
            Token::CloseParenthesis | Token::CloseSquareBracket => {
                let close = if matches!(token, Token::CloseParenthesis) {
                    ')'
                } else {
                    ']'
                };
                invalid |= stack.pop() != Some(close);
            }
            Token::CurlyBracketBlock if protected || (value && declaration) => stack.push('v'),
            Token::CloseCurlyBracket if protected => {
                invalid |= stack.pop() != Some('v');
            }
            Token::CurlyBracketBlock => {
                stack.push('}');
                value = false;
                statement = index + 1;
                declaration = is_declaration(pieces.get(statement..).unwrap_or_default());
                line_start = true;
            }
            Token::Colon if !protected && declaration => value = true,
            Token::Semicolon | Token::CloseCurlyBracket if !protected => {
                value = false;
                statement = index + 1;
                declaration = is_declaration(pieces.get(statement..).unwrap_or_default());
                line_start = true;
            }
            _ => {}
        }
    }
    invalid |= !stack.is_empty();
    Ok(Formatted {
        styled: out,
        notices: invalid
            .then(|| Notice::warning("Incomplete or invalid CSS; showing the available prefix. CSS property values are not validated."))
            .into_iter()
            .collect(),
    })
}

fn emit(out: &mut Styled, piece: &Piece<'_>, value: bool, declaration: bool) {
    if let Token::Dimension { unit, .. } = &piece.token {
        let split = piece.raw.len().saturating_sub(unit.len());
        if piece.raw.is_char_boundary(split) && piece.raw.ends_with(unit.as_ref()) {
            let (number, unit) = piece.raw.split_at(split);
            out.push(number, TokenKind::Number);
            out.push(unit, TokenKind::Literal);
        } else {
            out.push(piece.raw, TokenKind::Number);
        }
    } else {
        out.push(piece.raw, kind(&piece.token, value, declaration));
    }
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
    use crate::ui::body_format::NoticeLevel;

    #[test]
    fn nested_rules_and_declarations_have_indented_lines() -> Result<(), String> {
        let out = pretty(
            "@media screen{a:hover{color:#fff;width:2px;display:block;&:focus{color: red}}}",
        )?;
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
        Ok(())
    }

    #[test]
    fn preserves_strings_urls_comments_and_escapes() -> Result<(), String> {
        let text = r#"a{content:"a;{}\"b";background:url(data:image/svg+xml;a{b});x:url( "a;b" );}/*# sourceMappingURL=a.map */"#;
        let pieces = lex(text);
        assert_eq!(pieces.iter().map(|p| p.raw).collect::<String>(), text);
        let out = pretty(text)?;
        for protected in [
            r#""a;{}\"b""#,
            "url(data:image/svg+xml;a{b})",
            "url( \"a;b\" )",
            "/*# sourceMappingURL=a.map */",
        ] {
            assert!(out.styled.text().contains(protected), "{protected}");
        }
        Ok(())
    }

    #[test]
    fn multiline_comments_and_escaped_identifiers_keep_their_spelling() -> Result<(), String> {
        let text = "/* first\n second */a\\:b{content:\"한글\";color:rebeccapurple!important}";
        let out = pretty(text)?;
        assert!(out.styled.text().contains("/* first\n second */"));
        assert!(out.styled.text().contains("a\\:b"));
        assert!(
            out.styled
                .pieces()
                .any(|(s, k)| s.contains("rebeccapurple") && k == TokenKind::Literal)
        );
        assert_eq!(out.notices, []);
        Ok(())
    }

    #[test]
    fn custom_property_blocks_keep_internal_whitespace() -> Result<(), String> {
        let out = pretty("a{--theme:{a:b;c:d};color:var(--theme)}")?;
        assert!(out.styled.text().contains("--theme:{a:b;c:d};"));
        assert_eq!(out.notices, []);
        Ok(())
    }

    #[test]
    fn partial_and_bad_input_keeps_available_text() -> Result<(), String> {
        for text in [
            "a{color:red",
            "a{content:\"abc",
            "/* unfinished",
            "a{color:red)}",
            "a{color red;}",
        ] {
            let out = pretty(text)?;
            assert!(
                matches!(out.notices.as_slice(), [notice] if notice.level == NoticeLevel::Warning),
                "{text}"
            );
            assert_ne!(out.styled.text(), "");
        }
        Ok(())
    }
}
