//! Builds the [`TreeModel`] of a JSON body within node and depth limits.
use super::json::{self, JsonFormatError, Lexeme};
use super::tree::{CopyLabels, TreeModel, TreeNode, elide};
use super::{Styled, TokenKind};
use serde_json::Value;

const MAX_NODES: usize = 4096;
const MAX_DEPTH: usize = 64;

const COPY: CopyLabels = CopyLabels {
    path: "Copy path",
    value: "Copy value (JSON)",
};

/// Reads `text` as one JSON value. Member order, duplicate keys and number
/// spelling are kept as written.
pub(super) fn build(text: &str) -> Result<TreeModel, JsonFormatError> {
    let mut parser = Parser {
        text,
        offset: 0,
        tokens: json::lex(text)?.into_iter().peekable(),
        nodes: Vec::new(),
    };
    parser.value("$".into(), "$", None, 0)?;
    if parser.tokens.peek().is_some() || !parser.rest()?.trim_matches(json_whitespace).is_empty() {
        return Err(JsonFormatError::TrailingData);
    }
    Ok(TreeModel::new(parser.nodes, None, COPY))
}

// Tokens and source slices preserve member order, duplicate keys and number spelling.
// Only string keys are decoded, for constructing semantic paths.
struct Parser<'a> {
    text: &'a str,
    offset: usize,
    tokens: std::iter::Peekable<std::vec::IntoIter<Lexeme<'a>>>,
    nodes: Vec<TreeNode>,
}

impl<'a> Parser<'a> {
    fn rest(&self) -> Result<&str, JsonFormatError> {
        self.text
            .get(self.offset..)
            .ok_or(JsonFormatError::InvalidOffset)
    }

    fn take(&mut self) -> Result<Lexeme<'a>, JsonFormatError> {
        let token = self.tokens.next().ok_or(JsonFormatError::UnfinishedValue)?;
        let rest = self.rest()?;
        let trimmed = rest.trim_start_matches(json_whitespace);
        if !trimmed.starts_with(token.text()) {
            return Err(JsonFormatError::InvalidWhitespace);
        }
        self.offset += rest.len() - trimmed.len() + token.text().len();
        Ok(token)
    }

    fn expect(&mut self, expected: Lexeme<'_>) -> Result<(), JsonFormatError> {
        if self.take()? == expected {
            Ok(())
        } else {
            Err(JsonFormatError::UnexpectedToken)
        }
    }

    fn value(
        &mut self,
        key: String,
        path: &str,
        parent: Option<usize>,
        depth: usize,
    ) -> Result<(), JsonFormatError> {
        if depth > MAX_DEPTH || self.nodes.len() >= MAX_NODES {
            return Err(JsonFormatError::TreeLimitExceeded {
                max_depth: MAX_DEPTH,
                max_nodes: MAX_NODES,
            });
        }
        let start = self.offset + self.rest()?.len()
            - self.rest()?.trim_start_matches(json_whitespace).len();
        let token = self.take()?;
        let kind = match token {
            Lexeme::Open(_) => TokenKind::Punctuation,
            Lexeme::String(text) => {
                serde_json::from_str::<String>(text)?;
                TokenKind::String
            }
            Lexeme::Number(text) if valid_number(text) => TokenKind::Number,
            Lexeme::Literal("true" | "false" | "null") => TokenKind::Literal,
            _ => return Err(JsonFormatError::InvalidValue),
        };
        let open = match token {
            Lexeme::Open(open) => Some(open),
            _ => None,
        };
        let id = self.nodes.len();
        self.nodes.push(TreeNode {
            label: Styled::default(),
            name: key,
            path: Some(path.into()),
            value: String::new(),
            summary: None,
            parent,
            depth,
        });
        let count = if let Some(open) = open {
            self.children(open == "{", id, path, depth)?
        } else {
            0
        };
        let raw = self
            .text
            .get(start..self.offset)
            .ok_or(JsonFormatError::InvalidSpan)?;
        let summary = match open {
            Some("{") => format!("{{{count} keys}}"),
            Some(_) => format!("[{count} items]"),
            None => raw.into(),
        };
        let node = self.nodes.get_mut(id).ok_or(JsonFormatError::MissingNode)?;
        node.label.push(&format!("{}:", node.name), TokenKind::Key);
        node.label.push(" ", TokenKind::Text);
        node.label.push(&elide(&summary), kind);
        node.value = raw.into();
        Ok(())
    }

    fn children(
        &mut self,
        object: bool,
        parent: usize,
        path: &str,
        depth: usize,
    ) -> Result<usize, JsonFormatError> {
        let close = Lexeme::Close(if object { "}" } else { "]" });
        if self.tokens.peek() == Some(&close) {
            self.expect(close)?;
            return Ok(0);
        }
        let mut count = 0;
        loop {
            let (key, child_path) = if object {
                let Lexeme::String(raw) = self.take()? else {
                    return Err(JsonFormatError::ExpectedKey);
                };
                let key: String = serde_json::from_str(raw)?;
                let display = raw.to_owned();
                let child_path = child_path(path, &key);
                self.expect(Lexeme::Colon)?;
                (display, child_path)
            } else {
                (format!("[{count}]"), format!("{path}[{count}]"))
            };
            self.value(key, &child_path, Some(parent), depth + 1)?;
            count += 1;
            match self.tokens.peek() {
                Some(token) if *token == close => {
                    self.expect(close)?;
                    return Ok(count);
                }
                Some(Lexeme::Comma) => {
                    self.expect(Lexeme::Comma)?;
                }
                _ => return Err(JsonFormatError::ExpectedSeparatorOrClose),
            }
        }
    }
}

const fn json_whitespace(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\r')
}

/// Validate JSON number grammar without converting or rounding its value.
fn valid_number(text: &str) -> bool {
    let rest = text.strip_prefix('-').unwrap_or(text);
    let rest = if let Some(rest) = rest.strip_prefix('0') {
        rest
    } else {
        let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
        if digits == 0 {
            return false;
        }
        rest.get(digits..).unwrap_or_default()
    };
    let rest = if let Some(rest) = rest.strip_prefix('.') {
        let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
        if digits == 0 {
            return false;
        }
        rest.get(digits..).unwrap_or_default()
    } else {
        rest
    };
    let rest = if let Some(rest) = rest.strip_prefix(['e', 'E']) {
        let rest = rest.strip_prefix(['+', '-']).unwrap_or(rest);
        let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
        if digits == 0 {
            return false;
        }
        rest.get(digits..).unwrap_or_default()
    } else {
        rest
    };
    rest.is_empty()
}

fn child_path(parent: &str, key: &str) -> String {
    let mut chars = key.chars();
    let identifier = chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_' || c == '$')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$');
    if identifier {
        format!("{parent}.{key}")
    } else {
        format!("{parent}[{}]", Value::String(key.into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::tree::ROW_TEXT;
    fn summary(node: &TreeNode) -> &str {
        node.label
            .text()
            .strip_prefix(&format!("{}: ", node.name))
            .unwrap_or_default()
    }

    #[test]
    fn summaries_and_paths() -> Result<(), Box<dyn std::error::Error>> {
        let tree = build(r#"{"data":{"items":[{"id":42}]},"a.b":"long"}"#)?;
        assert_eq!(tree.nodes().first().map(summary), Some("{2 keys}"));
        assert_eq!(
            tree.nodes().get(2).map(summary),
            Some("[1 items]"),
            "arrays count items"
        );
        assert_eq!(child_path("$", "a.b"), "$[\"a.b\"]");
        let id = tree
            .nodes()
            .iter()
            .find(|node| node.value == "42")
            .ok_or("missing leaf")?;
        assert_eq!(id.path.as_deref(), Some("$.data.items[0].id"));
        assert_eq!(id.label.text(), "\"id\": 42");
        assert_eq!(tree.child_count(0), 2);
        Ok(())
    }

    #[test]
    fn elides_long_values_only_on_the_row() -> Result<(), Box<dyn std::error::Error>> {
        let long = "x".repeat(ROW_TEXT * 2);
        let tree = build(&format!("[\"{long}\"]"))?;
        let leaf = tree.node(1).ok_or("missing leaf")?;
        assert!(summary(leaf).ends_with('…'));
        assert_eq!(summary(leaf).chars().count(), ROW_TEXT + 1);
        assert_eq!(leaf.value, format!("\"{long}\""));
        Ok(())
    }

    #[test]
    fn preserves_member_order_duplicates_and_source_values() -> Result<(), JsonFormatError> {
        let text =
            r#"{ "b":12345678901234567890123, "a":1e2, "b":-0, "nested": {"z": 1E+9999, "z":2} }"#;
        let tree = build(text)?;
        assert_eq!(
            tree.nodes()
                .iter()
                .map(|node| node.name.as_str())
                .collect::<Vec<_>>(),
            [
                "$",
                "\"b\"",
                "\"a\"",
                "\"b\"",
                "\"nested\"",
                "\"z\"",
                "\"z\""
            ]
        );
        assert_eq!(
            tree.nodes()
                .iter()
                .map(|node| node.value.as_str())
                .collect::<Vec<_>>(),
            [
                text,
                "12345678901234567890123",
                "1e2",
                "-0",
                r#"{"z": 1E+9999, "z":2}"#,
                "1E+9999",
                "2"
            ]
        );
        assert_eq!(tree.nodes().first().map(summary), Some("{4 keys}"));
        assert_eq!(
            tree.nodes()
                .iter()
                .filter(|node| node.path.as_deref() == Some("$.b"))
                .count(),
            2
        );
        Ok(())
    }

    #[test]
    fn validates_grammar_without_numeric_conversion() -> Result<(), JsonFormatError> {
        for text in [
            "0",
            "-0",
            "1e2",
            "-12.30E-002",
            "12345678901234567890123",
            "1e99999",
        ] {
            let tree = build(text)?;
            assert_eq!(
                tree.nodes().first().map(|node| node.value.as_str()),
                Some(text)
            );
        }
        for text in [
            "01",
            "-",
            "1.",
            "1e",
            "1e+",
            "1e2.3",
            "--1",
            "true false",
            "[1 2]",
            "{true:2}",
            r#"{"a":1,}"#,
            r#"{"a" 1}"#,
            "[}",
            "[tr",
            "\"bad\\x\"",
            "\u{a0}0",
            "0\u{a0}",
            "[0,\u{a0}1]",
        ] {
            assert!(build(text).is_err(), "accepted {text:?}");
        }
        let tree = build(r#"{"\u0061.b": ["\u0061",false,null]}"#)?;
        assert_eq!(
            tree.nodes().get(1).and_then(|node| node.path.as_deref()),
            Some("$[\"a.b\"]")
        );
        assert_eq!(
            tree.nodes().get(2).map(|node| node.value.as_str()),
            Some(r#""\u0061""#)
        );
        Ok(())
    }

    #[test]
    fn rejects_invalid_truncated_and_excessive_input() {
        for text in [
            "{",
            "[true,]",
            &format!("{}0{}", "[".repeat(150), "]".repeat(150)),
            &format!("[{}0]", "0,".repeat(MAX_NODES)),
        ] {
            assert!(build(text).is_err());
        }
    }
}
