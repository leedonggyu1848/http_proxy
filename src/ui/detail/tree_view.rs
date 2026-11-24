//! Draws any [`TreeModel`] as collapsible rows: only the rows scrolled into
//! view are drawn, the open state survives theme and font changes, and
//! every row can be searched and its path or value copied.

use crate::ui::detail::highlight::{self, SyntaxPalette};
use crate::ui::style::theme::Theme;
use eframe::egui::{self, FontId, text::LayoutJob};
use http_proxy::content::format::tree::TreeModel;
use std::sync::Arc;

/// Levels open when a tree is first shown.
const OPEN_LEVELS: usize = 2;
/// Deepest level that still adds indentation, so rows stay on screen.
const MAX_INDENT: usize = 64;
/// Height of the row list before it scrolls.
const ROWS_HEIGHT: f32 = 350.0;

/// Row layouts for one theme and font.
struct Labels {
    theme: Theme,
    font: FontId,
    jobs: Vec<Arc<LayoutJob>>,
}

/// A tree with what the user did to it: open rows and the search.
pub(in crate::ui) struct TreeView {
    model: Arc<TreeModel>,
    open: Vec<bool>,
    matched: Vec<bool>,
    query: String,
    labels: Option<Labels>,
}

impl TreeView {
    pub(in crate::ui) fn new(model: Arc<TreeModel>) -> Self {
        let open = model
            .nodes()
            .iter()
            .map(|node| node.depth < OPEN_LEVELS)
            .collect();
        let matched = vec![false; model.nodes().len()];
        Self {
            model,
            open,
            matched,
            query: String::new(),
            labels: None,
        }
    }

    pub(super) fn displays(&self, model: &Arc<TreeModel>) -> bool {
        Arc::ptr_eq(&self.model, model)
    }

    fn expandable(&self, index: usize) -> bool {
        self.model.child_count(index) > 0
    }

    fn is_open(&self, index: usize) -> bool {
        self.open.get(index).copied().unwrap_or_default()
    }

    fn set_all(&mut self, open: bool) {
        self.open.fill(open);
    }

    #[cfg(test)]
    pub(in crate::ui) fn collapse_all(&mut self) {
        self.set_all(false);
    }

    #[cfg(test)]
    pub(in crate::ui) fn open_rows(&self) -> Vec<bool> {
        self.open.clone()
    }

    fn toggle(&mut self, index: usize) {
        if let Some(open) = self.open.get_mut(index) {
            *open = !*open;
        }
    }

    /// Marks the rows matching `query`, ignoring case, and opens their
    /// ancestors so every match is visible.
    fn search(&mut self, query: &str) {
        query.clone_into(&mut self.query);
        let query = query.to_lowercase();
        for (index, node) in self.model.nodes().iter().enumerate() {
            let leaf = self.model.child_count(index) == 0;
            let matched = !query.is_empty()
                && (node.name.to_lowercase().contains(&query)
                    || (leaf && node.value.to_lowercase().contains(&query)));
            if let Some(slot) = self.matched.get_mut(index) {
                *slot = matched;
            }
            if matched {
                let mut parent = node.parent;
                while let Some(id) = parent {
                    if let Some(open) = self.open.get_mut(id) {
                        *open = true;
                    }
                    parent = self.model.node(id).and_then(|node| node.parent);
                }
            }
        }
    }

    /// Rows not hidden under a closed row, in order.
    fn visible(&self) -> Vec<usize> {
        let mut visible = Vec::new();
        let mut hidden_below = None;
        for (index, node) in self.model.nodes().iter().enumerate() {
            if hidden_below.is_some_and(|depth| node.depth > depth) {
                continue;
            }
            hidden_below = (self.expandable(index) && !self.is_open(index)).then_some(node.depth);
            visible.push(index);
        }
        visible
    }

    /// Lays the rows out again only when `theme` or `font` changed.
    fn prepare_labels(&mut self, ctx: &egui::Context, theme: Theme, font: &FontId) {
        if self
            .labels
            .as_ref()
            .is_some_and(|labels| labels.theme == theme && labels.font == *font)
        {
            return;
        }
        let palette = SyntaxPalette::for_theme(theme);
        // The keyed theme's style: after a theme switch this frame's style
        // still belongs to the previous theme.
        let text_color = ctx.style_of(theme).visuals.text_color();
        let jobs = self
            .model
            .nodes()
            .iter()
            .map(|node| {
                Arc::new(highlight::layout_job(
                    ctx,
                    &node.label,
                    &palette,
                    text_color,
                    font,
                ))
            })
            .collect();
        self.labels = Some(Labels {
            theme,
            font: font.clone(),
            jobs,
        });
    }

    /// Shows the controls and the visible rows, colored for `theme`.
    pub(in crate::ui) fn show(&mut self, ui: &mut egui::Ui, theme: Theme, font: &FontId) {
        ui.weak("Right-click a node to copy its path or value.");
        ui.horizontal_wrapped(|ui| {
            if ui.button("Expand all").clicked() {
                self.set_all(true);
            }
            if ui.button("Collapse all").clicked() {
                self.set_all(false);
            }
            ui.label("Search:");
            let mut query = self.query.clone();
            let search_width = ui.available_width().clamp(100.0, 180.0);
            if ui
                .add(egui::TextEdit::singleline(&mut query).desired_width(search_width))
                .changed()
            {
                self.search(&query);
            }
        });
        self.prepare_labels(ui.ctx(), theme, font);
        let visible = self.visible();
        let height = ui
            .spacing()
            .interact_size
            .y
            .max(ui.text_style_height(&egui::TextStyle::Monospace));
        egui::ScrollArea::both()
            .id_salt("tree_rows")
            .max_height(ROWS_HEIGHT)
            .show_rows(ui, height, visible.len(), |ui, range| {
                for row in range {
                    if let Some(&index) = visible.get(row) {
                        self.row(ui, index);
                    }
                }
            });
    }

    fn row(&mut self, ui: &mut egui::Ui, index: usize) {
        let Some(node) = self.model.node(index) else {
            return;
        };
        let Some(job) = self
            .labels
            .as_ref()
            .and_then(|labels| labels.jobs.get(index))
        else {
            return;
        };
        let expandable = self.expandable(index);
        let open = self.is_open(index);
        let matched = self.matched.get(index).copied().unwrap_or_default();
        let mut toggled = false;
        ui.horizontal(|ui| {
            toggled = tree_row_prefix(ui, node.depth, expandable, open);
            let job = if matched {
                Arc::new(highlighted(job, ui.visuals().selection.bg_fill))
            } else {
                Arc::clone(job)
            };
            let response = ui
                .add(
                    egui::Label::new(egui::WidgetText::LayoutJob(job))
                        .wrap_mode(egui::TextWrapMode::Extend)
                        .sense(egui::Sense::click()),
                )
                .on_hover_text(&node.value);
            if expandable
                && !open
                && let Some(summary) = &node.summary
            {
                ui.weak(summary.as_str());
            }
            let copy = self.model.copy;
            response.context_menu(|ui| {
                if let Some(path) = &node.path {
                    ui.weak(path.as_str());
                    if ui.button(copy.path).clicked() {
                        ui.ctx().copy_text(path.clone());
                        ui.close();
                    }
                }
                if ui.button(copy.value).clicked() {
                    ui.ctx().copy_text(node.value.clone());
                    ui.close();
                }
            });
        });
        if toggled {
            self.toggle(index);
        }
    }
}

/// Draws indentation and the expand/collapse disclosure control shared by
/// body trees and the URL map tree. Returns `true` when the toggle was clicked.
pub(in crate::ui) fn tree_row_prefix(
    ui: &mut egui::Ui,
    depth: usize,
    expandable: bool,
    open: bool,
) -> bool {
    let clamped = u16::try_from(depth.min(MAX_INDENT)).unwrap_or(0);
    ui.add_space(f32::from(clamped) * 12.0);
    if expandable {
        ui.small_button(if open { "▼" } else { "▶" }).clicked()
    } else {
        ui.add_space(20.0);
        false
    }
}

/// `job` with every section drawn on `background`, for search matches.
fn highlighted(job: &LayoutJob, background: egui::Color32) -> LayoutJob {
    let mut job = job.clone();
    for section in &mut job.sections {
        section.format.background = background;
    }
    job
}

#[cfg(test)]
mod tests {
    use super::*;
    use http_proxy::content::format::{BodyFormat, tree};

    fn view(format: BodyFormat, text: &str) -> Result<TreeView, String> {
        tree::build(format, text)
            .ok_or("no tree")?
            .map(|model| TreeView::new(Arc::new(model)))
            .map_err(|error| error.to_string())
    }

    fn paths(view: &TreeView, indexes: &[usize]) -> Vec<String> {
        indexes
            .iter()
            .filter_map(|&index| view.model.node(index)?.path.clone())
            .collect()
    }

    #[test]
    fn search_opens_the_rows_above_each_match() -> Result<(), String> {
        let mut view = view(
            BodyFormat::Json,
            r#"{"data":{"items":[{"id":42}]},"a.b":"long"}"#,
        )?;
        view.set_all(false);
        assert_eq!(view.visible(), [0]);
        view.search("42");
        let matched = view
            .matched
            .iter()
            .position(|matched| *matched)
            .ok_or("missing match")?;
        assert_eq!(paths(&view, &[matched]), ["$.data.items[0].id"]);
        assert!(view.visible().contains(&matched));
        view.search("");
        assert!(view.matched.iter().all(|matched| !matched));
        Ok(())
    }

    #[test]
    fn search_matches_containers_by_name_only() -> Result<(), String> {
        let mut view = view(BodyFormat::Json, r#"{"keys":{"x":1},"y":"keys"}"#)?;
        view.search("KEYS");
        assert_eq!(
            paths(
                &view,
                &(0..view.matched.len())
                    .filter(|&index| view.matched.get(index) == Some(&true))
                    .collect::<Vec<_>>()
            ),
            ["$.keys", "$.y"],
            "the root's `{{2 keys}}` summary is not searched"
        );
        Ok(())
    }

    #[test]
    fn xml_trees_search_expand_and_collapse() -> Result<(), String> {
        let mut view = view(
            BodyFormat::Xml,
            "<rss><channel><item><title>Alpha</title></item><item><title>Beta</title></item></channel></rss>",
        )?;
        assert_eq!(
            paths(&view, &view.visible()),
            [
                "/rss",
                "/rss/channel",
                "/rss/channel/item[1]",
                "/rss/channel/item[2]"
            ],
            "two levels start open"
        );
        view.set_all(true);
        assert_eq!(view.visible().len(), 6);
        view.set_all(false);
        assert_eq!(view.visible(), [0]);
        view.search("beta");
        assert_eq!(
            paths(&view, &view.visible()),
            [
                "/rss",
                "/rss/channel",
                "/rss/channel/item[1]",
                "/rss/channel/item[2]",
                "/rss/channel/item[2]/title"
            ]
        );
        Ok(())
    }

    #[test]
    fn theme_change_relays_labels_and_keeps_open_rows() -> Result<(), String> {
        let ctx = egui::Context::default();
        let font = FontId::monospace(12.0);
        let mut view = view(BodyFormat::Json, r#"{"a":{"b":{"c":1}}}"#)?;
        view.set_all(false);
        view.toggle(0);
        view.prepare_labels(&ctx, Theme::Dark, &font);
        let color = |view: &TreeView| {
            view.labels
                .as_ref()
                .and_then(|labels| labels.jobs.first())
                .and_then(|job| job.sections.first())
                .map(|section| section.format.color)
        };
        let dark = color(&view);
        view.prepare_labels(&ctx, Theme::Light, &font);
        assert_ne!(color(&view), dark, "labels follow the theme");
        assert_eq!(view.open, [true, false, false, false]);
        Ok(())
    }
}
