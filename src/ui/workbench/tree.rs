//! Rows of trees and key-value inspectors (D18, D21, D24).
//!
//! A row is [`Layout::TREE_ROW_HEIGHT`] tall unless its value wraps. Each
//! level of depth indents it by [`Layout::TREE_INDENT`] and draws a 1px
//! `borders.subtle` guide under the chevron of the level above. Expandable
//! rows show a [`Layout::CHEVRON_SIZE`] chevron in `text.muted`, the
//! inactive icon color (D8). A selected row has the `surfaces.selected`
//! background and the 2px `accent.primary` bar of a selected table row
//! (D23); a row related to the selection elsewhere, or a search match, has
//! the `surfaces.cross_highlight` background (D21).

use eframe::egui;

use crate::ui::style::fonts;
use crate::ui::style::theme::Palette;
use crate::ui::style::tokens::{Layout, Space, StrokeWidth, ThemeTokens};

/// Deepest level that still adds indentation, so rows stay on screen.
const MAX_INDENT: usize = 64;

/// Whether a row has children, and whether they are shown.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Disclosure {
    Leaf,
    Open,
    Closed,
}

impl Disclosure {
    pub(crate) const fn new(expandable: bool, open: bool) -> Self {
        match (expandable, open) {
            (false, _) => Self::Leaf,
            (true, true) => Self::Open,
            (true, false) => Self::Closed,
        }
    }

    const fn expanded(self) -> Option<bool> {
        match self {
            Self::Leaf => None,
            Self::Open => Some(true),
            Self::Closed => Some(false),
        }
    }
}

/// How the background of a row marks it.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum Mark {
    #[default]
    None,
    /// The selected row of this tree.
    Selected,
    /// Related to the selection in another pane, or a search match.
    Highlighted,
}

/// One row: its depth, chevron, mark, and whether it reacts to the pointer.
#[derive(Clone, Copy, Debug)]
pub(crate) struct TreeRow<'a> {
    depth: usize,
    disclosure: Disclosure,
    mark: Mark,
    clickable: bool,
    label: &'a str,
}

impl<'a> TreeRow<'a> {
    /// A row at `depth` named `label` for assistive technology.
    pub(crate) const fn new(depth: usize, label: &'a str) -> Self {
        Self {
            depth,
            disclosure: Disclosure::Leaf,
            mark: Mark::None,
            clickable: false,
            label,
        }
    }

    pub(crate) const fn disclosure(mut self, disclosure: Disclosure) -> Self {
        self.disclosure = disclosure;
        self
    }

    pub(crate) const fn mark(mut self, mark: Mark) -> Self {
        self.mark = mark;
        self
    }

    /// Makes the whole row a click target with a hover background.
    pub(crate) const fn clickable(mut self) -> Self {
        self.clickable = true;
        self
    }
}

/// What happened to a row this frame.
pub(crate) struct RowResponse {
    pub(crate) response: egui::Response,
    /// The chevron was clicked, or the focused row got ←, →, Enter, or Space
    /// in the direction that changes it.
    pub(crate) toggled: bool,
}

/// Left edge of a row's content at `depth`: past the indentation and the
/// chevron column.
pub(crate) fn content_left(depth: usize) -> f32 {
    chevron_left(depth) + Layout::CHEVRON_SIZE + Space::XS
}

fn chevron_left(depth: usize) -> f32 {
    let clamped = u16::try_from(depth.min(MAX_INDENT)).unwrap_or(0);
    f32::from(clamped).mul_add(Layout::TREE_INDENT, Space::SM)
}

/// Shows one row across the available width and lays `add` out left to
/// right inside it, after the chevron.
pub(crate) fn row(
    ui: &mut egui::Ui,
    tree_row: TreeRow<'_>,
    add: impl FnOnce(&mut egui::Ui),
) -> RowResponse {
    let top_left = ui.cursor().min;
    let width = ui.available_width();
    let background = ui.painter().add(egui::Shape::Noop);
    let content = egui::Rect::from_min_size(
        egui::pos2(top_left.x + content_left(tree_row.depth), top_left.y),
        egui::vec2(
            (width - content_left(tree_row.depth)).max(0.0),
            Layout::TREE_ROW_HEIGHT,
        ),
    );
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(content)
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    child.set_min_height(Layout::TREE_ROW_HEIGHT);
    add(&mut child);
    let height = child.min_rect().height().max(Layout::TREE_ROW_HEIGHT);
    let rect = egui::Rect::from_min_size(top_left, egui::vec2(width, height));
    let sense = if tree_row.clickable {
        egui::Sense::click()
    } else {
        egui::Sense::hover()
    };
    let spacing = ui.spacing().item_spacing.y;
    ui.spacing_mut().item_spacing.y = 0.0;
    let response = ui.allocate_rect(rect, sense);
    ui.spacing_mut().item_spacing.y = spacing;
    announce(ui, &response, tree_row);
    let toggled = toggled(ui, &response, tree_row);
    if ui.is_rect_visible(rect) {
        let hovered = tree_row.clickable && response.contains_pointer();
        paint_background(ui, background, rect, tree_row.mark, hovered);
        paint_guides(ui, rect, tree_row.depth);
        paint_chevron(ui, rect, tree_row.depth, tree_row.disclosure);
        if response.has_focus() {
            crate::ui::workbench::controls::paint_focus_ring(ui, rect.shrink(1.0));
        }
    }
    RowResponse { response, toggled }
}

fn announce(ui: &egui::Ui, response: &egui::Response, tree_row: TreeRow<'_>) {
    let label = tree_row.label;
    response.widget_info(|| {
        egui::WidgetInfo::selected(
            egui::Role::TreeItem,
            ui.is_enabled(),
            tree_row.mark == Mark::Selected,
            label,
        )
    });
    ui.ctx().accesskit_node_builder(response.id, |node| {
        node.clear_toggled();
        if let Some(expanded) = tree_row.disclosure.expanded() {
            node.set_expanded(expanded);
        }
        node.set_selected(tree_row.mark == Mark::Selected);
    });
}

/// Whether the chevron was clicked, or the focused row was opened or closed
/// from the keyboard.
fn toggled(ui: &egui::Ui, response: &egui::Response, tree_row: TreeRow<'_>) -> bool {
    let Some(open) = tree_row.disclosure.expanded() else {
        return false;
    };
    if response.clicked()
        && let Some(pointer) = response.interact_pointer_pos()
        && chevron_hit(response.rect, tree_row.depth).contains(pointer)
    {
        return true;
    }
    if !response.has_focus() {
        return false;
    }
    ui.input(|input| {
        input.key_pressed(egui::Key::Enter)
            || input.key_pressed(egui::Key::Space)
            || (open && input.key_pressed(egui::Key::ArrowLeft))
            || (!open && input.key_pressed(egui::Key::ArrowRight))
    })
}

/// The chevron's click target: the chevron column across the row height.
fn chevron_hit(rect: egui::Rect, depth: usize) -> egui::Rect {
    let left = rect.left() + chevron_left(depth);
    egui::Rect::from_x_y_ranges(
        (left - Space::XS)..=(left + Layout::CHEVRON_SIZE + Space::XS),
        rect.top()..=(rect.top() + Layout::TREE_ROW_HEIGHT),
    )
}

fn paint_background(
    ui: &egui::Ui,
    slot: egui::layers::ShapeIdx,
    rect: egui::Rect,
    mark: Mark,
    hovered: bool,
) {
    let tokens = ThemeTokens::for_theme(ui.ctx().theme());
    let fill = match mark {
        Mark::Selected => Some(tokens.surfaces.selected),
        Mark::Highlighted => Some(tokens.surfaces.cross_highlight),
        Mark::None => hovered.then_some(tokens.surfaces.hover),
    };
    if let Some(fill) = fill {
        ui.painter()
            .set(slot, egui::Shape::rect_filled(rect, 0.0, fill));
    }
    if mark == Mark::Selected {
        let bar = egui::Rect::from_min_size(
            rect.min,
            egui::vec2(Layout::SELECTION_BAR_WIDTH, rect.height()),
        );
        ui.painter().rect_filled(bar, 0.0, tokens.accent.primary);
    }
}

/// One 1px guide per ancestor level, under that level's chevron.
fn paint_guides(ui: &egui::Ui, rect: egui::Rect, depth: usize) {
    let stroke = egui::Stroke::new(StrokeWidth::STRUCTURAL, Palette::of(ui).borders.subtle);
    for level in 0..depth.min(MAX_INDENT) {
        let x = (rect.left() + chevron_left(level) + Layout::CHEVRON_SIZE / 2.0).round() + 0.5;
        ui.painter().vline(x, rect.y_range(), stroke);
    }
}

/// A small filled triangle in the chevron column: pointing down when open,
/// right when closed.
fn paint_chevron(ui: &egui::Ui, rect: egui::Rect, depth: usize, disclosure: Disclosure) {
    let Some(open) = disclosure.expanded() else {
        return;
    };
    let color = ThemeTokens::for_theme(ui.ctx().theme()).text.muted;
    let center = egui::pos2(
        rect.left() + chevron_left(depth) + Layout::CHEVRON_SIZE / 2.0,
        rect.top() + Layout::TREE_ROW_HEIGHT / 2.0,
    );
    let half = Layout::CHEVRON_SIZE / 4.0;
    let points = if open {
        vec![
            center + egui::vec2(-half, -half / 2.0),
            center + egui::vec2(half, -half / 2.0),
            center + egui::vec2(0.0, half),
        ]
    } else {
        vec![
            center + egui::vec2(-half / 2.0, -half),
            center + egui::vec2(half, 0.0),
            center + egui::vec2(-half / 2.0, half),
        ]
    };
    ui.painter().add(egui::Shape::convex_polygon(
        points,
        color,
        egui::Stroke::NONE,
    ));
}

/// A group row at depth 0 that opens and closes the rows `add` shows under
/// it at depth 1. Groups start open; the state is kept per `id_salt`.
/// Returns the group row's response.
pub(crate) fn group(
    ui: &mut egui::Ui,
    id_salt: &str,
    title: &str,
    count: Option<usize>,
    add: impl FnOnce(&mut egui::Ui),
) -> egui::Response {
    let id = ui.make_persistent_id(id_salt);
    let mut open = ui.data_mut(|data| *data.get_persisted_mut_or(id, true));
    let shown = row(
        ui,
        TreeRow::new(0, title)
            .disclosure(Disclosure::new(true, open))
            .clickable(),
        |ui| group_title(ui, title, count),
    );
    if shown.response.clicked() || shown.toggled {
        open = !open;
        ui.data_mut(|data| data.insert_persisted(id, open));
    }
    if open {
        add(ui);
    }
    shown.response
}

/// The title of a group row: 11pt sans in `text.secondary`, followed by a
/// monospace count of its children (D15).
pub(crate) fn group_title(ui: &mut egui::Ui, title: &str, count: Option<usize>) {
    let secondary = Palette::of(ui).secondary;
    ui.add(
        egui::Label::new(
            egui::RichText::new(title)
                .font(fonts::column_header())
                .color(secondary)
                .strong(),
        )
        .selectable(false),
    );
    if let Some(count) = count {
        ui.add(
            egui::Label::new(
                egui::RichText::new(count.to_string())
                    .font(fonts::status())
                    .color(secondary),
            )
            .selectable(false),
        );
    }
}

/// A key, a `:` in `text.muted`, and a value, with the key in a column of
/// `key_width` so values line up (D24). Keys are `text.secondary` sans and
/// values `text.primary` monospace unless the caller styles them. Both wrap.
pub(crate) fn key_value(
    ui: &mut egui::Ui,
    key_width: f32,
    key: impl Into<egui::WidgetText>,
    value: impl Into<egui::WidgetText>,
) {
    ui.with_layout(egui::Layout::left_to_right(egui::Align::Min), |ui| {
        ui.add_space(0.0);
        ui.allocate_ui_with_layout(
            egui::vec2(key_width, 0.0),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                ui.set_width(key_width);
                ui.add(egui::Label::new(key).wrap());
            },
        );
        let muted = ThemeTokens::for_theme(ui.ctx().theme()).text.muted;
        ui.add(
            egui::Label::new(egui::RichText::new(":").font(fonts::data()).color(muted))
                .selectable(false),
        );
        let width = (ui.available_width() - Space::MD).max(Layout::DETAIL_VALUE_MIN_WIDTH);
        // Values wrap at spaces; a token without spaces, such as a URL,
        // still breaks inside the value column because egui falls back to
        // any position when a word does not fit on its row.
        let job = value.into().into_layout_job(
            ui.style(),
            egui::FontSelection::Default,
            egui::Align::Min,
        );
        ui.allocate_ui_with_layout(
            egui::vec2(width, 0.0),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                ui.set_width(width);
                ui.add(egui::Label::new(job).wrap());
            },
        );
    });
}

/// The key text of [`key_value`]: sans in `text.secondary`.
pub(crate) fn key_text(ui: &egui::Ui, key: &str) -> egui::RichText {
    egui::RichText::new(key)
        .font(fonts::control())
        .color(Palette::of(ui).secondary)
}

/// The value text of [`key_value`]: monospace in `text.primary`.
pub(crate) fn value_text(ui: &egui::Ui, value: &str) -> egui::RichText {
    egui::RichText::new(value)
        .font(fonts::data())
        .color(Palette::of(ui).text)
}

/// Width of the key column for a pane `available` points wide.
pub(crate) fn key_width(available: f32) -> f32 {
    Layout::DETAIL_KEY_WIDTH.fit(available * 0.3)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_level_indents_by_the_tree_indent() {
        assert!((content_left(1) - content_left(0) - Layout::TREE_INDENT).abs() < f32::EPSILON);
        assert!(
            (content_left(0) - (Space::SM + Layout::CHEVRON_SIZE + Space::XS)).abs() < f32::EPSILON
        );
        assert!(
            (content_left(MAX_INDENT + 10) - content_left(MAX_INDENT)).abs() < f32::EPSILON,
            "very deep rows stop indenting"
        );
    }

    #[test]
    fn rows_are_tree_height_and_report_expansion() -> Result<(), Box<dyn std::error::Error>> {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let mut rects = Vec::new();
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            for (depth, disclosure) in [(0, Disclosure::Open), (1, Disclosure::Leaf)] {
                let shown = row(
                    ui,
                    TreeRow::new(depth, "row")
                        .disclosure(disclosure)
                        .clickable(),
                    |ui| {
                        ui.label("text");
                    },
                );
                rects.push(shown.response.rect);
            }
        });
        output.textures_delta.clear();
        for rect in &rects {
            assert!(
                (rect.height() - Layout::TREE_ROW_HEIGHT).abs() < 0.5,
                "{rect:?}"
            );
        }
        let [first, second] = rects.as_slice() else {
            return Err("expected two rows".into());
        };
        assert!(
            (second.top() - first.bottom()).abs() < 0.5,
            "rows meet without a gap"
        );
        let tree = output
            .platform_output
            .accesskit_update
            .ok_or("missing accessibility tree")?;
        let mut items: Vec<_> = tree
            .nodes
            .iter()
            .filter(|(_, node)| node.role() == egui::Role::TreeItem)
            .map(|(_, node)| node.is_expanded())
            .collect();
        items.sort_unstable();
        assert_eq!(items, [None, Some(true)]);
        Ok(())
    }

    /// Rows of the painted galley whose text is `text`, as strings.
    fn painted_rows(shapes: &[egui::epaint::ClippedShape], text: &str) -> Vec<(String, f32)> {
        shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                egui::Shape::Text(shape) if shape.galley.text() == text => Some(shape),
                _ => None,
            })
            .flat_map(|shape| {
                shape.galley.rows.iter().map(|row| {
                    let line: String = row.row.glyphs.iter().map(|glyph| glyph.chr).collect();
                    (line, shape.pos.x + row.rect().right())
                })
            })
            .collect()
    }

    #[test]
    fn values_wrap_at_spaces_and_long_tokens_stay_in_the_column() {
        const SPACED: &str = "Failed at Tunnel relay: Connection reset by peer (os error 104)";
        const TOKEN: &str = "span-0123456789abcdefspan0123456789abcdefspan0123456789abcdef";
        let ctx = egui::Context::default();
        let mut value_column = 0.0;
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            ui.allocate_ui(egui::vec2(320.0, 400.0), |ui| {
                ui.set_width(320.0);
                for value in [SPACED, TOKEN] {
                    key_value(
                        ui,
                        96.0,
                        "key",
                        egui::RichText::new(value).font(fonts::data()),
                    );
                }
                value_column = ui.max_rect().right();
            });
        });
        output.textures_delta.clear();
        let spaced = painted_rows(&output.shapes, SPACED);
        assert!(spaced.len() > 1, "the value wraps: {spaced:?}");
        let mut consumed = 0;
        for (line, _) in &spaced {
            consumed += line.len();
            let next = SPACED.get(consumed..consumed + 1).unwrap_or(" ");
            let last = line.chars().last().unwrap_or(' ');
            assert!(
                last == ' ' || next == " ",
                "row {line:?} breaks inside a word"
            );
        }
        let token = painted_rows(&output.shapes, TOKEN);
        assert!(token.len() > 1, "a long token wraps: {token:?}");
        for (line, right) in &token {
            assert!(
                *right <= value_column + 0.5,
                "{line:?} runs past the column"
            );
        }
    }
}
