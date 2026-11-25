pub(super) mod capture;
mod controller;
mod dialogs;
pub(super) mod state;

pub(super) use controller::AppAction;

use self::state::{
    DetailState, Dialogs, LayoutState, MainView, MapState, RequestsState, SessionState,
};
use crate::ui::detail::detail_view::DetailView;
use crate::ui::map::{MapAction, MapSubView};
use crate::ui::requests::{filter::ListView, request_table::RequestTable};
use crate::ui::style::fonts;
use crate::ui::style::theme::{self, Palette};
use crate::ui::style::tokens::{Layout, Radius, Space};

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
        let dialog_open =
            body_dialog_open || self.dialogs.har.is_open() || self.detail.replay_is_open();
        self.update(ui.ctx());
        let actions = self.view(ui, body_dialog_open, dialog_open);
        self.dispatch(ui.ctx(), actions);
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
                let top = ui.cursor().top();
                self.toolbar(ui, &mut actions);
                self.notices(ui, &mut actions);
                ui.separator();
                self.view_switcher(ui, &mut actions);
                match self.layout.main_view {
                    MainView::Requests => {
                        self.request_area(
                            ui,
                            viewport_height,
                            top,
                            shortcuts_allowed,
                            &mut actions,
                        );
                    }
                    MainView::Map => {
                        if let Some(action) = self.map.panel.controls(
                            ui,
                            self.map.analysis.url_map(),
                            self.dialogs.save.is_busy(),
                        ) {
                            pending_map_action = Some(action);
                        }
                        let used = ui.cursor().top() - top + ui.spacing().item_spacing.y;
                        let height =
                            main_height(viewport_height, used, self.layout.status_bar_height);
                        if let Some(action) = self.map_area(ui, height, shortcuts_allowed) {
                            pending_map_action = Some(action);
                        }
                    }
                }
                let status_top = ui.cursor().top();
                ui.separator();
                self.status_bar(ui, &mut actions);
                self.layout.status_bar_height = ui.min_rect().bottom() - status_top;
            });

        actions.extend(pending_map_action.map(AppAction::Map));
        actions
    }

    fn view_switcher(&self, ui: &mut egui::Ui, actions: &mut Vec<AppAction>) {
        ui.horizontal_wrapped(|ui| {
            let req_label = format!("Requests ({})", self.requests.records.len());
            let mut view = self.layout.main_view;
            ui.selectable_value(&mut view, MainView::Requests, req_label);
            let map_label = format!(
                "Map ({} hosts · {} nodes)",
                self.map.analysis.url_map().origins().len(),
                self.map.analysis.url_map().node_count()
            );
            ui.selectable_value(&mut view, MainView::Map, map_label);
            if view != self.layout.main_view {
                actions.push(AppAction::SwitchView(view));
            }
        });
    }

    /// Requests and details are clipped to disjoint panes.
    fn request_area(
        &mut self,
        ui: &mut egui::Ui,
        viewport_height: f32,
        top: f32,
        shortcuts_allowed: bool,
        actions: &mut Vec<AppAction>,
    ) {
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
        };
        requests.filters(ui, &mut self.request_table, actions);
        let used = ui.cursor().top() - top + ui.spacing().item_spacing.y;
        let height = main_height(viewport_height, used, self.layout.status_bar_height);
        split_panes(
            ui,
            height,
            &mut self.layout.detail_share,
            ("main-area", "main-split", "requests-pane", "details-pane"),
            |left_ui, right_ui| {
                let selected = requests
                    .table(left_ui, &mut self.request_table, shortcuts_allowed, actions)
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
                    curl: &self.detail.curl,
                    body_saving: &self.session.body_saving,
                };
                detail.show(
                    right_ui,
                    &mut self.detail.widgets,
                    shortcuts_allowed,
                    actions,
                );
            },
        );
    }

    /// Map main area: split tree + detail panes in Tree sub-view, or full-width
    /// relationship graph canvas in Graph sub-view.
    fn map_area(
        &mut self,
        ui: &mut egui::Ui,
        height: f32,
        shortcuts_allowed: bool,
    ) -> Option<MapAction> {
        if self.map.panel.sub_view == MapSubView::Graph {
            let size = egui::vec2(ui.available_width(), height);
            let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
            let mut main = ui.new_child(
                egui::UiBuilder::new()
                    .id_salt("map-graph-area")
                    .max_rect(rect)
                    .layout(egui::Layout::top_down(egui::Align::Min)),
            );
            main.shrink_clip_rect(rect);
            self.map
                .panel
                .show_graph_pane(&mut main, self.map.analysis.url_map());
            return None;
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
                self.map.panel.show_tree_pane(
                    left_ui,
                    self.map.analysis.url_map(),
                    shortcuts_allowed,
                );
                self.map.panel.show_detail_pane(
                    right_ui,
                    self.map.analysis.url_map(),
                    &self.requests.records,
                    self.map.analysis.endpoint_schemas(),
                )
            },
        );
        self.layout.detail_share = share;
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
    if let Some(prev) = main.ctx().read_response(split_id)
        && (prev.dragged() || prev.drag_stopped())
        && let Some(pointer) = prev.interact_pointer_pos()
        && width > 0.0
    {
        let (min_detail, max_detail) = detail_width_bounds(width);
        let dragged = (rect.right() - pointer.x).clamp(min_detail, max_detail);
        *detail_share = (dragged / width).clamp(0.0, 1.0);
    }

    let detail_width = split_detail_width(width, *detail_share);
    let split_x = (rect.right() - detail_width)
        .round()
        .clamp(rect.left(), rect.right());
    let left_rect = egui::Rect::from_min_max(
        rect.min,
        egui::pos2(
            (split_x - Layout::SPLIT_GUTTER_HALF_WIDTH).max(rect.left()),
            rect.max.y,
        ),
    );
    let right_outer = egui::Rect::from_min_max(
        egui::pos2(
            (split_x + Layout::SPLIT_GUTTER_HALF_WIDTH).min(rect.right()),
            rect.min.y,
        ),
        rect.max,
    );
    let right_rect = egui::Rect::from_min_max(
        egui::pos2(
            (right_outer.left() + Space::SM).min(right_outer.right()),
            right_outer.top(),
        ),
        right_outer.max,
    );

    let mut left_ui = main.new_child(
        egui::UiBuilder::new()
            .id_salt(ids.2)
            .max_rect(left_rect)
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    left_ui.shrink_clip_rect(left_rect);

    let mut right_ui = main.new_child(
        egui::UiBuilder::new()
            .id_salt(ids.3)
            .max_rect(right_rect)
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    right_ui.shrink_clip_rect(right_rect);
    paint_pane_surfaces(&main, [left_rect, right_outer]);
    let result = render(&mut left_ui, &mut right_ui);

    let handle_rect = egui::Rect::from_x_y_ranges(
        (split_x - Layout::SPLIT_HANDLE_HALF_WIDTH).max(rect.left())
            ..=(split_x + Layout::SPLIT_HANDLE_HALF_WIDTH).min(rect.right()),
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
    let stroke = theme::divider_stroke(&main, &handle);
    main.painter().vline(split_x, rect.y_range(), stroke);
    result
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

/// Height left for the request list and details once the controls above and
/// the status bar below are placed, but never less than [`MIN_MAIN_HEIGHT`].
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
