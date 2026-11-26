//! Draws any [`TreeModel`] as collapsible rows: only the rows scrolled into
//! view are drawn, the open state survives theme and font changes, and
//! every row can be searched and its path or value copied.

use crate::ui::detail::highlight;
use crate::ui::style::fonts;
use crate::ui::style::theme::{self, Palette, SyntaxPalette, Theme};
use crate::ui::style::tokens::Layout;
use crate::ui::workbench::field::{self, Field};
use crate::ui::workbench::text::ShellText;
use crate::ui::workbench::{controls, tree};
use eframe::egui::{self, FontId, text::LayoutJob};
use http_inspector_content::format::tree::TreeModel;
use std::sync::Arc;

/// Levels open when a tree is first shown.
const OPEN_LEVELS: usize = 2;

/// Row layouts for one theme and font.
struct Labels {
    theme: Theme,
    font: FontId,
    jobs: Vec<Arc<LayoutJob>>,
}

/// A tree with what the user did to it: open rows and the search.
pub(super) struct TreeView {
    model: Arc<TreeModel>,
    open: Vec<bool>,
    matched: Vec<bool>,
    query: String,
    labels: Option<Labels>,
}

impl TreeView {
    pub(super) fn new(model: Arc<TreeModel>) -> Self {
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
    pub(super) fn collapse_all(&mut self) {
        self.set_all(false);
    }

    #[cfg(test)]
    pub(super) fn open_rows(&self) -> Vec<bool> {
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
        let text_color = theme::text_in_theme(ctx, theme);
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
    pub(super) fn show(
        &mut self,
        ui: &mut egui::Ui,
        theme: Theme,
        font: &FontId,
        lang: crate::ui::i18n::Language,
    ) -> Option<String> {
        self.controls(ui, lang);
        self.prepare_labels(ui.ctx(), theme, font);
        let visible = self.visible();
        let mut copied = None;
        egui::ScrollArea::both()
            .id_salt("tree_rows")
            .max_height(Layout::DETAIL_TREE_HEIGHT)
            .auto_shrink([false, true])
            .show_rows(ui, Layout::TREE_ROW_HEIGHT, visible.len(), |ui, range| {
                ui.spacing_mut().item_spacing.y = 0.0;
                for row in range {
                    if let Some(&index) = visible.get(row)
                        && let Some(text) = self.row(ui, index, lang)
                    {
                        copied = Some(text);
                    }
                }
            });
        copied
    }

    /// Expand all, Collapse all, and the key filter, in one pane row.
    fn controls(&mut self, ui: &mut egui::Ui, lang: crate::ui::i18n::Language) {
        egui::Frame::new()
            .inner_margin(Layout::PANE_ROW_MARGIN)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    controls::ghost_style(ui);
                    let mut query = self.query.clone();
                    let width = Layout::TREE_SEARCH_WIDTH.fit(ui.available_width());
                    let field = Field::new(width)
                        .hint(ShellText::FilterKeys.text(lang))
                        .search_icon();
                    if field::single_line(ui, &mut query, field, |_| {}).changed() {
                        self.search(&query);
                    }
                    for (label, open) in [
                        (ShellText::ExpandAll, true),
                        (ShellText::CollapseAll, false),
                    ] {
                        if ui
                            .add(controls::ghost(
                                label.text(lang),
                                Layout::PANE_BUTTON_HEIGHT,
                            ))
                            .clicked()
                        {
                            self.set_all(open);
                        }
                    }
                });
            });
    }

    fn row(
        &mut self,
        ui: &mut egui::Ui,
        index: usize,
        lang: crate::ui::i18n::Language,
    ) -> Option<String> {
        let node = self.model.node(index)?;
        let job = Arc::clone(self.labels.as_ref()?.jobs.get(index)?);
        let open = self.is_open(index);
        let mark = if self.matched.get(index).copied().unwrap_or_default() {
            tree::Mark::Highlighted
        } else {
            tree::Mark::None
        };
        let summary = (self.expandable(index) && !open)
            .then_some(node.summary.as_deref())
            .flatten();
        let shown = tree::row(
            ui,
            tree::TreeRow::new(node.depth, &node.name)
                .disclosure(tree::Disclosure::new(self.expandable(index), open))
                .mark(mark)
                .clickable(),
            |ui| {
                ui.add(
                    egui::Label::new(egui::WidgetText::LayoutJob(job))
                        .wrap_mode(egui::TextWrapMode::Extend)
                        .selectable(false),
                );
                if let Some(summary) = summary {
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(summary)
                                .font(fonts::data())
                                .color(Palette::of(ui).secondary),
                        )
                        .selectable(false),
                    );
                }
            },
        );
        let mut copied = None;
        let copy = self.model.copy;
        let response = shown.response.on_hover_text(&node.value);
        response.context_menu(|ui| {
            if let Some(path) = &node.path {
                ui.weak(path.as_str());
                if ui.button(tree_copy_label(lang, copy.path)).clicked() {
                    copied = Some(path.clone());
                    ui.close();
                }
            }
            if ui.button(tree_copy_label(lang, copy.value)).clicked() {
                copied = Some(node.value.clone());
                ui.close();
            }
        });
        if self.expandable(index) && (response.clicked() || shown.toggled) {
            self.toggle(index);
        }
        copied
    }
}

fn tree_copy_label(lang: crate::ui::i18n::Language, raw: &'static str) -> &'static str {
    match lang {
        crate::ui::i18n::Language::English => raw,
        crate::ui::i18n::Language::Korean => match raw {
            "Copy JSONPath" => "JSONPath 복사",
            "Copy XPath" => "XPath 복사",
            "Copy path" => "경로 복사",
            "Copy value" => "값 복사",
            other => other,
        },
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use http_inspector_content::format::{BodyFormat, tree};

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
        let font = fonts::code();
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
