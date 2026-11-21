//! Draws an [`XmlTree`] as collapsible rows whose element paths can be
//! copied.

use crate::ui::body_format::highlight::{self, SyntaxPalette};
use crate::ui::body_format::xml::tree::{TreeNode, XmlTree};
use eframe::egui::{self, Color32, FontId, collapsing_header::CollapsingState, text::LayoutJob};
use std::sync::Arc;

/// Element levels open when a tree is first shown.
const OPEN_LEVELS: usize = 2;

/// A tree with its rows laid out for one theme and font.
pub(in crate::ui) struct XmlTreeView {
    tree: XmlTree,
    labels: Vec<Arc<LayoutJob>>,
}

impl XmlTreeView {
    pub(in crate::ui) fn new(
        tree: XmlTree,
        palette: &SyntaxPalette,
        text_color: Color32,
        font_id: &FontId,
    ) -> Self {
        let labels = tree
            .nodes
            .iter()
            .map(|node| {
                Arc::new(highlight::layout_job(
                    &node.label,
                    palette,
                    text_color,
                    font_id,
                ))
            })
            .collect();
        Self { tree, labels }
    }

    pub(in crate::ui) fn incomplete(&self) -> Option<&str> {
        self.tree.incomplete.as_deref()
    }

    /// Shows the rows; `id_salt` keeps the open state of each body apart.
    pub(in crate::ui) fn show(
        &self,
        ui: &mut egui::Ui,
        id_salt: impl std::hash::Hash + std::fmt::Debug,
    ) {
        ui.weak("Right-click an element to copy its XPath.");
        let id = ui.make_persistent_id(id_salt);
        self.rows(ui, id, &self.tree.roots);
    }

    fn rows(&self, ui: &mut egui::Ui, id: egui::Id, indexes: &[usize]) {
        for &index in indexes {
            let (Some(node), Some(label)) = (self.tree.node(index), self.labels.get(index)) else {
                continue;
            };
            if node.children.is_empty() {
                ui.horizontal(|ui| {
                    ui.add_space(ui.spacing().icon_width + ui.spacing().icon_spacing);
                    row_label(ui, node, label, None);
                });
                continue;
            }
            let state = CollapsingState::load_with_default_open(
                ui.ctx(),
                id.with(index),
                node.depth < OPEN_LEVELS,
            );
            let collapsed = (!state.is_open()).then_some(node.children.len());
            state
                .show_header(ui, |ui| row_label(ui, node, label, collapsed))
                .body(|ui| self.rows(ui, id, &node.children));
        }
    }
}

/// Draws one row; `collapsed` is the child count of a closed element.
fn row_label(ui: &mut egui::Ui, node: &TreeNode, label: &Arc<LayoutJob>, collapsed: Option<usize>) {
    let text = egui::WidgetText::LayoutJob(Arc::clone(label));
    let response = ui.add(
        egui::Label::new(text)
            .truncate()
            .sense(egui::Sense::click()),
    );
    if let Some(count) = collapsed {
        let noun = if count == 1 { "child" } else { "children" };
        ui.weak(format!("… {count} {noun}"));
    }
    if let Some(path) = &node.path {
        response.context_menu(|ui| {
            ui.weak(path.as_str());
            if ui.button("Copy XPath").clicked() {
                ui.ctx().copy_text(path.clone());
                ui.close();
            }
        });
    }
}
