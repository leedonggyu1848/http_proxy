pub(super) mod capture;
mod controller;
mod dialogs;
pub(super) mod state;
mod status_bar;

pub(super) use controller::AppAction;

use self::state::{
    DetailState, Dialogs, LayoutState, MainView, MapState, RequestsState, SessionState,
};
use crate::ui::detail::detail_view::DetailView;
use crate::ui::i18n::Language;
use crate::ui::map::{MapAction, MapPanel, MapSubView};
use crate::ui::requests::{filter::ListView, request_table::RequestTable};
use crate::ui::style::fonts;
use crate::ui::style::theme::{self, Palette};
use crate::ui::style::tokens::{Layout, Radius, Space, StrokeWidth};

use eframe::egui;
use http_inspector_analysis::pipeline::AnalysisPipeline;
use http_inspector_core::interception::ProxyHandle;

pub(super) const MAX_RECORDS: usize = 2_000;
pub fn main() -> eframe::Result {
    init_tracing();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size(Layout::WINDOW)
            .with_min_inner_size(Layout::MIN_WINDOW),
        ..eframe::NativeOptions::default()
    };
    eframe::run_native(
        http_inspector_exchange::APP_NAME,
        options,
        Box::new(|context| {
            fonts::configure_fonts(&context.egui_ctx);
            Ok(Box::new(InspectorApp::new(context.egui_ctx.clone())))
        }),
    )
}

use tracing_subscriber::EnvFilter;

fn init_tracing() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn"));
    let _subscriber_already_set = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .try_init()
        .is_err();
}

pub(super) struct InspectorApp {
    pub(super) session: SessionState,
    pub(super) requests: RequestsState,
    request_table: RequestTable,
    pub(super) detail: DetailState,
    pub(super) map: MapState,
    pub(super) dialogs: Dialogs,
    pub(super) layout: LayoutState,
    context: egui::Context,
}

impl InspectorApp {
    fn new(context: egui::Context) -> Self {
        let layout = LayoutState::new(&context);
        let session = SessionState::new();
        let wake_context = context.clone();
        let analysis = AnalysisPipeline::start(session.body_source(), move || {
            wake_context.request_repaint();
        });
        let mut app = Self {
            session,
            requests: RequestsState::new(&context),
            request_table: RequestTable::new(),
            detail: DetailState::default(),
            map: MapState::new(analysis),
            dialogs: Dialogs::new(),
            layout,
            context,
        };
        app.start_proxy();
        app
    }
}

impl eframe::App for InspectorApp {
    fn clear_color(&self, visuals: &egui::Visuals) -> [f32; 4] {
        theme::clear_color(visuals)
    }

    fn ui(&mut self, ui: &mut eframe::egui::Ui, _frame: &mut eframe::Frame) {
        self.render_ui(ui);
    }
}

impl InspectorApp {
    /// Renders one frame of the inspector inside `ui`.
    pub(super) fn render_ui(&mut self, ui: &mut egui::Ui) {
        // The closing frame still belongs to the dialog that received the input.
        let body_dialog_open = self.session.body_saving.dialog_is_open();
        let dialog_open = body_dialog_open
            || self.dialogs.har.is_open()
            || self.dialogs.hls.is_open()
            || self.detail.replay_is_open();
        self.update(ui.ctx());
        let actions = self.view(ui, body_dialog_open, dialog_open);
        self.dispatch(ui.ctx(), actions);
    }

    pub(crate) const fn language(&self) -> crate::ui::i18n::Language {
        self.layout.language.language()
    }

    fn view(
        &mut self,
        ui: &mut egui::Ui,
        body_dialog_open: bool,
        dialog_open: bool,
    ) -> Vec<AppAction> {
        let mut actions = Vec::new();
        self.dialog_actions(ui.ctx(), body_dialog_open, &mut actions);
        let shortcuts_allowed = !dialog_open && request_shortcuts_allowed(ui.ctx());
        let viewport_height = ui.available_height();
        let mut pending_map_action: Option<MapAction> = None;
        egui::ScrollArea::vertical()
            .id_salt("window-scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                // Bands and panes meet at their 1px lines, with no gaps.
                ui.spacing_mut().item_spacing.y = 0.0;
                let top = ui.cursor().top();
                self.toolbar(ui, dialog_open, &mut actions);
                self.notices(ui, &mut actions);
                let shown = match self.layout.main_view {
                    MainView::Requests => Some(self.request_area(
                        ui,
                        viewport_height,
                        top,
                        shortcuts_allowed,
                        &mut actions,
                    )),
                    MainView::Map => {
                        let used = ui.cursor().top() - top;
                        let height = main_height(viewport_height, used, Layout::STATUS_BAR_HEIGHT);
                        if let Some(action) = self.map_area(ui, height, shortcuts_allowed) {
                            pending_map_action = Some(action);
                        }
                        None
                    }
                };
                self.status_bar(ui, shown, &mut actions);
            });

        actions.extend(pending_map_action.map(AppAction::Map));
        actions
    }

    /// Requests and details are clipped to disjoint panes.
    fn request_area(
        &mut self,
        ui: &mut egui::Ui,
        viewport_height: f32,
        top: f32,
        shortcuts_allowed: bool,
        actions: &mut Vec<AppAction>,
    ) -> usize {
        let lang = self.language();
        let rows = self.requests.filtered_records();
        let shown = rows.len();
        let requests = ListView {
            requests: &self.requests,
            analysis: &self.map.analysis,
            proxy: self
                .session
                .capture
                .proxy
                .as_ref()
                .map(ProxyHandle::local_addr),
            saving_body: self.session.body_saving.is_busy(),
            lang,
        };
        let used = ui.cursor().top() - top;
        let height = main_height(viewport_height, used, Layout::STATUS_BAR_HEIGHT);
        split_panes(
            ui,
            height,
            &mut self.layout.detail_share,
            ("main-area", "main-split", "requests-pane", "details-pane"),
            |left_ui, right_ui| {
                requests.header(left_ui, &mut self.request_table, actions);
                let selected = requests
                    .table(
                        left_ui,
                        &mut self.request_table,
                        rows,
                        shortcuts_allowed,
                        actions,
                    )
                    .or(self.requests.selected);
                let record = selected.and_then(|id| {
                    self.requests
                        .records
                        .iter()
                        .find(|record| record.id() == id)
                });
                let detail = DetailView {
                    record,
                    search: self.requests.search.active_query().map(AsRef::as_ref),
                    tab: self.detail.tab,
                    body_mode: self.detail.body_mode,
                    body_saving: &self.session.body_saving,
                    field_menu_enabled: self.requests.search.field_menu_enabled(),
                    lang,
                };
                detail.show(
                    right_ui,
                    &mut self.detail.widgets,
                    shortcuts_allowed,
                    actions,
                );
            },
        );
        shown
    }

    /// Map main area: split tree + detail panes in Tree sub-view, or full-width
    /// relationship graph canvas (with an optional docked right legend pane) in
    /// Graph sub-view. The pane header holds the view tabs and the map actions.
    fn map_area(
        &mut self,
        ui: &mut egui::Ui,
        height: f32,
        shortcuts_allowed: bool,
    ) -> Option<MapAction> {
        let lang = self.language();
        let save_busy = self.dialogs.save.is_busy();
        if self.map.panel.sub_view == MapSubView::Graph {
            return self.map_graph_area(ui, height, save_busy, lang);
        }

        let mut share = self.layout.detail_share;
        let action = split_panes(
            ui,
            height,
            &mut share,
            (
                "map-main-area",
                "map-split",
                "map-tree-pane",
                "map-detail-pane",
            ),
            |left_ui, right_ui| {
                let header =
                    self.map
                        .panel
                        .header(left_ui, self.map.analysis.url_map(), save_busy, lang);
                self.map.panel.tree_filters(left_ui, lang);
                self.map.panel.show_tree_pane(
                    left_ui,
                    self.map.analysis.url_map(),
                    shortcuts_allowed,
                    lang,
                );
                let detail = self.map.panel.show_detail_pane(
                    right_ui,
                    self.map.analysis.url_map(),
                    &self.requests.records,
                    self.map.analysis.endpoint_schemas(),
                    lang,
                );
                detail.or(header)
            },
        );
        self.layout.detail_share = share;
        action
    }

    fn map_graph_area(
        &mut self,
        ui: &mut egui::Ui,
        height: f32,
        save_busy: bool,
        lang: Language,
    ) -> Option<MapAction> {
        let size = egui::vec2(ui.available_width(), height);
        let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
        let mut main = ui.new_child(
            egui::UiBuilder::new()
                .id_salt("map-graph-area")
                .max_rect(rect)
                .layout(egui::Layout::top_down(egui::Align::Min)),
        );
        main.shrink_clip_rect(rect);
        main.spacing_mut().item_spacing.y = 0.0;
        paint_pane_surfaces(&main, [rect, egui::Rect::NOTHING]);
        let action = self
            .map
            .panel
            .header(&mut main, self.map.analysis.url_map(), save_busy, lang);
        let body_rect =
            egui::Rect::from_min_max(egui::pos2(rect.left(), main.cursor().top()), rect.max);
        let url_map = self.map.analysis.url_map();
        if self.map.panel.legend_open && can_dock_graph_legend(body_rect.width()) {
            let mut legend_width = self.map.panel.legend_width;
            split_graph_legend(
                &mut main,
                body_rect,
                &mut legend_width,
                |canvas_ui, legend_ui| {
                    self.map.panel.show_graph_pane(canvas_ui, url_map, lang);
                    MapPanel::show_graph_legend(legend_ui, lang);
                },
            );
            self.map.panel.legend_width = legend_width;
        } else {
            let mut canvas_ui = main.new_child(
                egui::UiBuilder::new()
                    .id_salt("map-graph-canvas")
                    .max_rect(body_rect)
                    .layout(egui::Layout::top_down(egui::Align::Min)),
            );
            canvas_ui.shrink_clip_rect(body_rect);
            self.map
                .panel
                .show_graph_pane(&mut canvas_ui, url_map, lang);
        }
        action
    }
}

/// Fills content panes one surface above the window chrome around them.
fn paint_pane_surfaces(ui: &egui::Ui, panes: [egui::Rect; 2]) {
    let fill = Palette::of(ui).surfaces.pane;
    for pane in panes {
        ui.painter().rect_filled(pane, Radius::PANE, fill);
    }
}

fn read_dragged_right_width(
    main: &egui::Ui,
    rect: egui::Rect,
    split_id: egui::Id,
    bounds: (f32, f32),
) -> Option<f32> {
    let prev = main.ctx().read_response(split_id)?;
    if !(prev.dragged() || prev.drag_stopped()) {
        return None;
    }
    let pointer = prev.interact_pointer_pos()?;
    let (min_right, max_right) = bounds;
    Some((rect.right() - pointer.x).clamp(min_right, max_right))
}

fn render_split_rect<R>(
    main: &mut egui::Ui,
    rect: egui::Rect,
    split_id: egui::Id,
    right_width: f32,
    pane_ids: (&str, &str),
    render: impl FnOnce(&mut egui::Ui, &mut egui::Ui) -> R,
) -> R {
    let split_x = (rect.right() - right_width)
        .round()
        .clamp(rect.left(), rect.right());
    // The 1px splitter line occupies [split_x, split_x + 1).
    let left_rect = egui::Rect::from_min_max(rect.min, egui::pos2(split_x, rect.max.y));
    let right_rect = egui::Rect::from_min_max(
        egui::pos2(
            (split_x + StrokeWidth::STRUCTURAL).min(rect.right()),
            rect.min.y,
        ),
        rect.max,
    );

    let mut left_ui = main.new_child(
        egui::UiBuilder::new()
            .id_salt(pane_ids.0)
            .max_rect(left_rect)
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    left_ui.shrink_clip_rect(left_rect);

    let mut right_ui = main.new_child(
        egui::UiBuilder::new()
            .id_salt(pane_ids.1)
            .max_rect(right_rect)
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    right_ui.shrink_clip_rect(right_rect);
    for pane in [&mut left_ui, &mut right_ui] {
        pane.spacing_mut().item_spacing.y = Space::XS;
    }
    paint_pane_surfaces(main, [left_rect, right_rect]);
    let result = render(&mut left_ui, &mut right_ui);

    let line_x = split_x + StrokeWidth::STRUCTURAL / 2.0;
    let handle_rect = egui::Rect::from_x_y_ranges(
        (line_x - Layout::SPLITTER_HIT / 2.0).max(rect.left())
            ..=(line_x + Layout::SPLITTER_HIT / 2.0).min(rect.right()),
        rect.y_range(),
    );
    let handle = main.interact(handle_rect, split_id, egui::Sense::drag());
    if handle.dragged() || handle.dragged_by(egui::PointerButton::Primary) || handle.drag_stopped()
    {
        main.ctx().request_repaint();
    }
    if handle.hovered() || handle.dragged() {
        main.ctx()
            .set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
    }
    let stroke = theme::divider_stroke(main, &handle);
    main.painter().vline(line_x, rect.y_range(), stroke);
    result
}

fn split_panes<R>(
    ui: &mut egui::Ui,
    height: f32,
    detail_share: &mut f32,
    ids: (&str, &str, &str, &str),
    render: impl FnOnce(&mut egui::Ui, &mut egui::Ui) -> R,
) -> R {
    let size = egui::vec2(ui.available_width(), height);
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    let mut main = ui.new_child(
        egui::UiBuilder::new()
            .id_salt(ids.0)
            .max_rect(rect)
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    main.shrink_clip_rect(rect);

    let width = rect.width();
    let split_id = main.make_persistent_id(ids.1);
    if width > 0.0
        && let Some(dragged) =
            read_dragged_right_width(&main, rect, split_id, detail_width_bounds(width))
    {
        *detail_share = (dragged / width).clamp(0.0, 1.0);
    }

    let detail_width = split_detail_width(width, *detail_share);
    render_split_rect(
        &mut main,
        rect,
        split_id,
        detail_width,
        (ids.2, ids.3),
        render,
    )
}

fn split_graph_legend<R>(
    parent: &mut egui::Ui,
    rect: egui::Rect,
    legend_width: &mut f32,
    render: impl FnOnce(&mut egui::Ui, &mut egui::Ui) -> R,
) -> R {
    let mut body = parent.new_child(
        egui::UiBuilder::new()
            .id_salt("map-graph-body")
            .max_rect(rect)
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    body.shrink_clip_rect(rect);

    let width = rect.width();
    let split_id = body.make_persistent_id("map-graph-legend-split");
    if width > 0.0
        && let Some(dragged) =
            read_dragged_right_width(&body, rect, split_id, graph_legend_width_bounds(width))
    {
        *legend_width = dragged;
    }

    let clamped = graph_legend_width(width, *legend_width);
    render_split_rect(
        &mut body,
        rect,
        split_id,
        clamped,
        ("map-graph-canvas", "map-graph-legend"),
        render,
    )
}

/// Minimum and maximum width of the right details pane for `total_width`.
fn detail_width_bounds(total_width: f32) -> (f32, f32) {
    let min_detail = Layout::MIN_DETAIL_WIDTH.min((total_width * 0.5).floor());
    let max_detail = (total_width - Layout::MIN_TABLE_WIDTH).max(min_detail);
    (min_detail, max_detail)
}

/// Width of the right details pane for `total_width` and preferred `share`.
fn split_detail_width(total_width: f32, share: f32) -> f32 {
    let (min_detail, max_detail) = detail_width_bounds(total_width);
    (total_width * share).clamp(min_detail, max_detail)
}

/// Minimum and maximum width of the docked graph legend pane (including the
/// 1px structural splitter) for `total_width`, keeping at least
/// [`Layout::GRAPH_CANVAS_MIN`] for the graph canvas.
fn graph_legend_width_bounds(total_width: f32) -> (f32, f32) {
    let max_for_canvas = (total_width - Layout::GRAPH_CANVAS_MIN - StrokeWidth::STRUCTURAL)
        .floor()
        .max(0.0);
    let max_legend = Layout::GRAPH_LEGEND_MAX_WIDTH.min(max_for_canvas);
    let min_legend = Layout::GRAPH_LEGEND_MIN_WIDTH.min(max_legend);
    (min_legend, max_legend)
}

/// Whether `total_width` has room for both the minimum graph canvas and a
/// readable docked legend pane without collapsing the pane to a 0px strip.
fn can_dock_graph_legend(total_width: f32) -> bool {
    let (_, max_legend) = graph_legend_width_bounds(total_width);
    max_legend >= Layout::GRAPH_LEGEND_MIN_WIDTH
}

/// Width of the docked graph legend pane for `total_width` and `preferred`.
fn graph_legend_width(total_width: f32, preferred: f32) -> f32 {
    let (min_legend, max_legend) = graph_legend_width_bounds(total_width);
    preferred.clamp(min_legend, max_legend)
}

/// Height left for the request list and details once the controls above and
/// the status bar below are placed, but never less than [`Layout::MIN_MAIN_HEIGHT`].
fn main_height(viewport: f32, above: f32, below: f32) -> f32 {
    (viewport - above - below)
        .floor()
        .max(Layout::MIN_MAIN_HEIGHT)
}

pub(super) fn request_shortcuts_allowed(ctx: &egui::Context) -> bool {
    let focused = ctx.memory(egui::Memory::focused);
    !egui::Popup::is_any_open(ctx)
        && focused.is_none_or(|id| egui::TextEdit::load_state(ctx, id).is_none())
}

#[cfg(test)]
mod layout_tests;
