//! Token-based JSON pretty printer that tolerates truncated input.

use super::{Styled, TokenKind, split_while};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Lexeme<'a> {
    Open(&'a str),
    Close(&'a str),
    Comma,
    Colon,
    String(&'a str),
    Number(&'a str),
    Literal(&'a str),
}

impl<'a> Lexeme<'a> {
    pub(super) const fn text(self) -> &'a str {
        match self {
            Self::Open(text)
            | Self::Close(text)
            | Self::String(text)
            | Self::Number(text)
            | Self::Literal(text) => text,
            Self::Comma => ",",
            Self::Colon => ":",
        }
    }
}

const LITERALS: [&str; 3] = ["true", "false", "null"];

/// Reason JSON input could not be formatted or parsed into a tree.
#[derive(Debug, thiserror::Error)]
pub enum JsonFormatError {
    #[error("unexpected {found:?} at byte {offset}")]
    UnexpectedChar { found: char, offset: usize },
    #[error("unexpected word {word:?} at byte {offset}")]
    UnexpectedWord { word: String, offset: usize },
    #[error("JSON preview nesting exceeds 64 levels")]
    PreviewNestingTooDeep,
    #[error("unexpected data after JSON value")]
    TrailingData,
    #[error("invalid JSON offset")]
    InvalidOffset,
    #[error("unfinished JSON value")]
    UnfinishedValue,
    #[error("invalid JSON whitespace")]
    InvalidWhitespace,
    #[error("unexpected JSON token")]
    UnexpectedToken,
    #[error("tree limit exceeded ({max_depth} levels, {max_nodes} nodes)")]
    TreeLimitExceeded { max_depth: usize, max_nodes: usize },
    #[error(transparent)]
    InvalidString(#[from] serde_json::Error),
    #[error("invalid JSON value")]
    InvalidValue,
    #[error("invalid JSON span")]
    InvalidSpan,
    #[error("missing JSON node")]
    MissingNode,
    #[error("expected JSON key")]
    ExpectedKey,
    #[error("expected comma or closing bracket")]
    ExpectedSeparatorOrClose,
}

/// Indents JSON two spaces per level. Unbalanced brackets and an unfinished
/// last token are shown as they are; characters that cannot start a JSON
/// token fail with their byte offset.
pub(super) fn pretty(text: &str) -> Result<Styled, JsonFormatError> {
    let lexemes = lex(text)?;
    let mut out = Styled::default();
    let mut depth = 0_usize;
    let mut lexemes = lexemes.into_iter().peekable();
    while let Some(lexeme) = lexemes.next() {
        let next = lexemes.peek().copied();
        match lexeme {
            Lexeme::Open(open) => {
                out.push(open, TokenKind::Punctuation);
                if let Some(Lexeme::Close(close)) = next {
                    out.push(close, TokenKind::Punctuation);
                    lexemes.next();
                } else {
                    depth += 1;
                    if depth > 64 {
                        return Err(JsonFormatError::PreviewNestingTooDeep);
                    }
                    if next.is_some() {
                        out.newline(depth);
                    }
                }
            }
            Lexeme::Close(close) => {
                depth = depth.saturating_sub(1);
                if !out.text().is_empty() {
                    out.newline(depth);
                }
                out.push(close, TokenKind::Punctuation);
            }
            Lexeme::Comma => {
                out.push(",", TokenKind::Punctuation);
                if next.is_some() {
                    out.newline(depth);
                }
            }
            Lexeme::Colon => {
                out.push(":", TokenKind::Punctuation);
                out.push(" ", TokenKind::Text);
            }
            Lexeme::String(string) => {
                let kind = if next == Some(Lexeme::Colon) {
                    TokenKind::Key
                } else {
                    TokenKind::String
                };
                out.push_quoted_multiline(string, kind);
            }
            Lexeme::Number(number) => out.push(number, TokenKind::Number),
            Lexeme::Literal(literal) => out.push(literal, TokenKind::Literal),
        }
    }
    Ok(out)
}

pub(super) fn lex(text: &str) -> Result<Vec<Lexeme<'_>>, JsonFormatError> {
    let mut lexemes = Vec::new();
    let mut rest = text.trim_start();
    while let Some(first) = rest.chars().next() {
        let offset = text.len() - rest.len();
        let (piece, after) = match first {
            '{' | '[' | '}' | ']' | ',' | ':' => rest.split_at(1),
            '"' => rest.split_at(string_len(rest)),
            '-' | '0'..='9' => split_while(rest, |c| {
                c.is_ascii_digit() || matches!(c, '-' | '+' | '.' | 'e' | 'E')
            }),
            'a'..='z' => split_while(rest, |c| c.is_ascii_alphabetic()),
            other => {
                return Err(JsonFormatError::UnexpectedChar {
                    found: other,
                    offset,
                });
            }
        };
        let lexeme = match first {
            '{' | '[' => Lexeme::Open(piece),
            '}' | ']' => Lexeme::Close(piece),
            ',' => Lexeme::Comma,
            ':' => Lexeme::Colon,
            '"' => Lexeme::String(piece),
            'a'..='z' if is_literal(piece, after.is_empty()) => Lexeme::Literal(piece),
            'a'..='z' => {
                return Err(JsonFormatError::UnexpectedWord {
                    word: piece.to_owned(),
                    offset,
                });
            }
            _ => Lexeme::Number(piece),
        };
        lexemes.push(lexeme);
        rest = after.trim_start();
    }
    Ok(lexemes)
}

/// Accepts JSON literals, and their prefixes when the input was cut there.
fn is_literal(word: &str, at_end: bool) -> bool {
    LITERALS
        .iter()
        .any(|literal| *literal == word || (at_end && literal.starts_with(word)))
}

/// Length of the string starting at the opening quote of `text`, including
/// the closing quote, or the rest of `text` when the string is unterminated.
fn string_len(text: &str) -> usize {
    let mut escaped = false;
    for (index, c) in text.char_indices().skip(1) {
        if escaped {
            escaped = false;
        } else if c == '\\' {
            escaped = true;
        } else if c == '"' {
            return index + 1;
        }
    }
    text.len()
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
    fn indents_minified_json_and_classifies_tokens() -> Result<(), JsonFormatError> {
        let styled =
            pretty(r#"{"name":"a\"b","n":-1.5e3,"ok":true,"none":null,"list":[1,{}],"empty":[]}"#)?;
        assert_eq!(
            styled.text(),
            "{\n  \"name\": \"a\\\"b\",\n  \"n\": -1.5e3,\n  \"ok\": true,\n  \"none\": null,\n  \"list\": [\n    1,\n    {}\n  ],\n  \"empty\": []\n}"
        );
        assert_eq!(
            kinds(&styled, TokenKind::Key),
            [
                "\"name\"",
                "\"n\"",
                "\"ok\"",
                "\"none\"",
                "\"list\"",
                "\"empty\""
            ]
        );
        assert_eq!(kinds(&styled, TokenKind::String), ["\"a\\\"b\""]);
        assert_eq!(kinds(&styled, TokenKind::Number), ["-1.5e3", "1"]);
        assert_eq!(kinds(&styled, TokenKind::Literal), ["true", "null"]);
        Ok(())
    }

    #[test]
    fn formats_truncated_json_as_far_as_it_goes() -> Result<(), JsonFormatError> {
        let styled = pretty(r#"{"items":[{"id":1,"name":"unfinish"#)?;
        assert_eq!(
            styled.text(),
            "{\n  \"items\": [\n    {\n      \"id\": 1,\n      \"name\": \"unfinish"
        );
        assert_eq!(pretty("[tr")?.text(), "[\n  tr");
        assert_eq!(pretty("]]")?.text(), "]\n]");
        assert_eq!(pretty("  ")?.text(), "");
        Ok(())
    }

    #[test]
    fn bounds_indentation_for_deep_tree_fallback() {
        let text = "[".repeat(4096);
        assert!(matches!(
            pretty(&text),
            Err(JsonFormatError::PreviewNestingTooDeep)
        ));
    }

    #[test]
    fn rejects_text_that_is_not_json() {
        let err = pretty("{\"a\": <b>}").err();
        assert!(matches!(
            err,
            Some(JsonFormatError::UnexpectedChar {
                found: '<',
                offset: 6
            })
        ));
        assert_eq!(
            err.as_ref().map(ToString::to_string).as_deref(),
            Some("unexpected '<' at byte 6")
        );
        assert!(matches!(
            pretty("hello"),
            Err(JsonFormatError::UnexpectedWord { ref word, offset: 0 }) if word == "hello"
        ));
        assert!(matches!(
            pretty("[tr, 1]"),
            Err(JsonFormatError::UnexpectedWord { ref word, offset: 1 }) if word == "tr"
        ));
    }

    #[test]
    fn aligns_multiline_json_strings_visually_without_changing_text() -> Result<(), JsonFormatError>
    {
        let input = "{\"outer\":{\"note\":\"first\nsecond\nthird\",\"next\":1}}";
        let styled = pretty(input)?;
        assert_eq!(
            styled.text(),
            "{\n  \"outer\": {\n    \"note\": \"first\nsecond\nthird\",\n    \"next\": 1\n  }\n}"
        );
        let align_for = |needle: &str| -> Option<&str> {
            let offset = styled.text().find(needle)?;
            styled
                .indents()
                .iter()
                .find(|indent| indent.offset == offset)
                .map(|indent| indent.align_to.as_str())
        };
        assert_eq!(align_for("second"), Some("    \"note\": \""));
        assert_eq!(align_for("third"), Some("    \"note\": \""));
        Ok(())
    }
}
