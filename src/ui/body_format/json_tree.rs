//! Bounded, cached JSON tree with virtualized visible rows.
use super::highlight::SyntaxPalette;
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
        // serde_json's recursion limit rejects pathological input before we recurse.
        let value: Value = serde_json::from_str(text).map_err(|error| error.to_string())?;
        let mut tree = Self {
            nodes: Vec::new(),
            query: String::new(),
        };
        tree.add(&value, "$".into(), "$", None, 0)?;
        Ok(tree)
    }

    fn add(
        &mut self,
        value: &Value,
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
        let (summary, kind) = match value {
            Value::Object(map) => (format!("{{{} keys}}", map.len()), Kind::Container),
            Value::Array(array) => (format!("[{} items]", array.len()), Kind::Container),
            Value::String(_) => (value.to_string(), Kind::String),
            Value::Number(_) => (value.to_string(), Kind::Number),
            _ => (value.to_string(), Kind::Literal),
        };
        let id = self.nodes.len();
        self.nodes.push(Node {
            key,
            path: path.into(),
            summary,
            value: value.to_string(),
            parent,
            depth,
            container: value.is_object() || value.is_array(),
            open: depth < 2,
            matched: false,
            kind,
        });
        match value {
            Value::Object(map) => {
                for (key, child) in map {
                    self.add(
                        child,
                        format!("{key:?}"),
                        &child_path(path, key),
                        Some(id),
                        depth + 1,
                    )?;
                }
            }
            Value::Array(array) => {
                for (index, child) in array.iter().enumerate() {
                    self.add(
                        child,
                        format!("[{index}]"),
                        &format!("{path}[{index}]"),
                        Some(id),
                        depth + 1,
                    )?;
                }
            }
            _ => {}
        }
        Ok(())
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
