//! Bounded, cached JSON tree with virtualized visible rows.
use super::highlight::SyntaxPalette;
use super::json::{self, Lexeme};
use eframe::egui::{self, RichText};
use serde_json::Value;

const MAX_NODES: usize = 4096;
const MAX_DEPTH: usize = 64;

struct Node {
    key: String,
    path: String,
    summary: String,
    value: String,
    parent: Option<usize>,
    depth: usize,
    container: bool,
    open: bool,
    matched: bool,
    kind: Kind,
}

#[derive(Clone, Copy)]
enum Kind {
    Container,
    String,
    Number,
    Literal,
}

pub(in crate::ui) struct Tree {
    nodes: Vec<Node>,
    query: String,
}

impl Tree {
    pub(in crate::ui) fn parse(text: &str) -> Result<Self, String> {
        let mut parser = Parser {
            text,
            offset: 0,
            tokens: json::lex(text)?.into_iter().peekable(),
            nodes: Vec::new(),
        };
        parser.value("$".into(), "$", None, 0)?;
        if parser.tokens.peek().is_some()
            || !parser.rest()?.trim_matches(json_whitespace).is_empty()
        {
            return Err("unexpected data after JSON value".into());
        }
        Ok(Self {
            nodes: parser.nodes,
            query: String::new(),
        })
    }

    fn search(&mut self) {
        let query = self.query.to_lowercase();
        for node in &mut self.nodes {
            node.matched = !query.is_empty()
                && (node.key.to_lowercase().contains(&query)
                    || (!node.container && node.value.to_lowercase().contains(&query)));
        }
        for id in 0..self.nodes.len() {
            if self.nodes.get(id).is_some_and(|node| node.matched) {
                let mut parent = self.nodes.get(id).and_then(|node| node.parent);
                while let Some(id) = parent {
                    let Some(node) = self.nodes.get_mut(id) else {
                        break;
                    };
                    node.open = true;
                    parent = node.parent;
                }
            }
        }
    }

    fn visible(&self) -> Vec<usize> {
        let mut visible = Vec::new();
        let mut hidden_below = None;
        for (id, node) in self.nodes.iter().enumerate() {
            if hidden_below.is_some_and(|depth| node.depth > depth) {
                continue;
            }
            hidden_below = if node.container && !node.open {
                Some(node.depth)
            } else {
                None
            };
            visible.push(id);
        }
        visible
    }

    pub(in crate::ui) fn show(&mut self, ui: &mut egui::Ui) {
        ui.weak("Format: JSON · Tree · Right-click a node to copy path or value");
        ui.horizontal(|ui| {
            if ui.button("Expand all").clicked() {
                for node in &mut self.nodes {
                    node.open = true;
                }
            }
            if ui.button("Collapse all").clicked() {
                for node in &mut self.nodes {
                    node.open = false;
                }
            }
            ui.label("Search:");
            if ui.text_edit_singleline(&mut self.query).changed() {
                self.search();
            }
        });
        let visible = self.visible();
        let colors = SyntaxPalette::for_theme(ui.ctx().theme());
        let height = ui
            .spacing()
            .interact_size
            .y
            .max(ui.text_style_height(&egui::TextStyle::Monospace));
        egui::ScrollArea::both()
            .id_salt("json_tree_rows")
            .max_height(350.0)
            .show_rows(ui, height, visible.len(), |ui, range| {
                for row in range {
                    let Some(node) = visible.get(row).and_then(|id| self.nodes.get_mut(*id)) else {
                        continue;
                    };
                    ui.horizontal(|ui| {
                        // Bound indentation even on small windows.
                        ui.add_space(
                            f32::from(u16::try_from(node.depth.min(MAX_DEPTH)).unwrap_or(0)) * 12.0,
                        );
                        if node.container {
                            if ui.small_button(if node.open { "▼" } else { "▶" }).clicked() {
                                node.open = !node.open;
                            }
                        } else {
                            ui.add_space(20.0);
                        }
                        ui.label(
                            RichText::new(format!("{}:", node.key))
                                .monospace()
                                .color(colors.key),
                        );
                        let color = match node.kind {
                            Kind::Container => colors.punctuation,
                            Kind::String => colors.string,
                            Kind::Number => colors.number,
                            Kind::Literal => colors.literal,
                        };
                        let short: String = node.summary.chars().take(100).collect();
                        let short = if node.summary.chars().count() > 100 {
                            format!("{short}…")
                        } else {
                            short
                        };
                        let mut label = RichText::new(short).monospace().color(color);
                        if node.matched {
                            label = label
                                .background_color(ui.visuals().selection.bg_fill)
                                .strong();
                        }
                        let response = ui.label(label).on_hover_text(&node.value);
                        response.context_menu(|ui| {
                            if ui.button("Copy path").clicked() {
                                ui.ctx().copy_text(node.path.clone());
                                ui.close();
                            }
                            if ui.button("Copy value (JSON)").clicked() {
                                ui.ctx().copy_text(node.value.clone());
                                ui.close();
                            }
                        });
                    });
                }
            });
    }
}

// Tokens and source slices preserve member order, duplicate keys and number spelling.
// Only string keys are decoded, for constructing semantic paths.
struct Parser<'a> {
    text: &'a str,
    offset: usize,
    tokens: std::iter::Peekable<std::vec::IntoIter<Lexeme<'a>>>,
    nodes: Vec<Node>,
}

impl<'a> Parser<'a> {
    fn rest(&self) -> Result<&str, String> {
        self.text
            .get(self.offset..)
            .ok_or_else(|| "invalid JSON offset".into())
    }

    fn take(&mut self) -> Result<Lexeme<'a>, String> {
        let token = self.tokens.next().ok_or("unfinished JSON value")?;
        let rest = self.rest()?;
        let trimmed = rest.trim_start_matches(json_whitespace);
        if !trimmed.starts_with(token.text()) {
            return Err("invalid JSON whitespace".into());
        }
        self.offset += rest.len() - trimmed.len() + token.text().len();
        Ok(token)
    }

    fn expect(&mut self, expected: Lexeme<'_>) -> Result<(), String> {
        if self.take()? == expected {
            Ok(())
        } else {
            Err("unexpected JSON token".into())
        }
    }

    fn value(
        &mut self,
        key: String,
        path: &str,
        parent: Option<usize>,
        depth: usize,
    ) -> Result<(), String> {
        if depth > MAX_DEPTH || self.nodes.len() >= MAX_NODES {
            return Err(format!(
                "tree limit exceeded ({MAX_DEPTH} levels, {MAX_NODES} nodes)"
            ));
        }
        let start = self.offset + self.rest()?.len()
            - self.rest()?.trim_start_matches(json_whitespace).len();
        let token = self.take()?;
        let kind = match token {
            Lexeme::Open(_) => Kind::Container,
            Lexeme::String(text) => {
                serde_json::from_str::<String>(text).map_err(|error| error.to_string())?;
                Kind::String
            }
            Lexeme::Number(text) if valid_number(text) => Kind::Number,
            Lexeme::Literal("true" | "false" | "null") => Kind::Literal,
            _ => return Err("invalid JSON value".into()),
        };
        let open = match token {
            Lexeme::Open(open) => Some(open),
            _ => None,
        };
        let id = self.nodes.len();
        self.nodes.push(Node {
            key,
            path: path.into(),
            summary: String::new(),
            value: String::new(),
            parent,
            depth,
            container: open.is_some(),
            open: depth < 2,
            matched: false,
            kind,
        });
        let count = if let Some(open) = open {
            self.children(open == "{", id, path, depth)?
        } else {
            0
        };
        let raw = self
            .text
            .get(start..self.offset)
            .ok_or("invalid JSON span")?;
        let node = self.nodes.get_mut(id).ok_or("missing JSON node")?;
        node.value = raw.into();
        node.summary = match open {
            Some("{") => format!("{{{count} keys}}"),
            Some(_) => format!("[{count} items]"),
            None => raw.into(),
        };
        Ok(())
    }

    fn children(
        &mut self,
        object: bool,
        parent: usize,
        path: &str,
        depth: usize,
    ) -> Result<usize, String> {
        let close = Lexeme::Close(if object { "}" } else { "]" });
        if self.tokens.peek() == Some(&close) {
            self.expect(close)?;
            return Ok(0);
        }
        let mut count = 0;
        loop {
            let (key, child_path) = if object {
                let Lexeme::String(raw) = self.take()? else {
                    return Err("expected JSON key".into());
                };
                let key: String = serde_json::from_str(raw).map_err(|error| error.to_string())?;
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
                _ => return Err("expected comma or closing bracket".into()),
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
    #[test]
    fn summaries_paths_and_search_expansion() -> Result<(), String> {
        let mut tree = Tree::parse(r#"{"data":{"items":[{"id":42}]},"a.b":"long"}"#)?;
        assert_eq!(
            tree.nodes.first().map(|n| n.summary.as_str()),
            Some("{2 keys}")
        );
        assert_eq!(child_path("$", "a.b"), "$[\"a.b\"]");
        for node in &mut tree.nodes {
            node.open = false;
        }
        assert_eq!(tree.visible().len(), 1);
        tree.query = "42".into();
        tree.search();
        let matched = tree
            .nodes
            .iter()
            .position(|n| n.matched)
            .ok_or("missing match")?;
        assert_eq!(
            tree.nodes.get(matched).map(|n| n.path.as_str()),
            Some("$.data.items[0].id")
        );
        assert!(tree.visible().contains(&matched));
        Ok(())
    }
    #[test]
    fn preserves_member_order_duplicates_and_source_values() -> Result<(), String> {
        let text =
            r#"{ "b":12345678901234567890123, "a":1e2, "b":-0, "nested": {"z": 1E+9999, "z":2} }"#;
        let tree = Tree::parse(text)?;
        assert_eq!(
            tree.nodes
                .iter()
                .map(|node| node.key.as_str())
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
            tree.nodes
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
        assert_eq!(
            tree.nodes.first().map(|node| node.summary.as_str()),
            Some("{4 keys}")
        );
        assert_eq!(
            tree.nodes.iter().filter(|node| node.path == "$.b").count(),
            2
        );
        Ok(())
    }

    #[test]
    fn validates_grammar_without_numeric_conversion() -> Result<(), String> {
        for text in [
            "0",
            "-0",
            "1e2",
            "-12.30E-002",
            "12345678901234567890123",
            "1e99999",
        ] {
            let tree = Tree::parse(text)?;
            assert_eq!(
                tree.nodes.first().map(|node| node.value.as_str()),
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
            assert!(Tree::parse(text).is_err(), "accepted {text:?}");
        }
        let tree = Tree::parse(r#"{"\u0061.b": ["\u0061",false,null]}"#)?;
        assert_eq!(
            tree.nodes.get(1).map(|node| node.path.as_str()),
            Some("$[\"a.b\"]")
        );
        assert_eq!(
            tree.nodes.get(2).map(|node| node.value.as_str()),
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
            assert!(Tree::parse(text).is_err());
        }
    }
}
