mod app_paths;
mod body_display;
mod body_format;
mod body_matches;
mod body_saving;
mod body_view;
mod browser_panel;
mod bulk_save;
mod ca_panel;
mod copy_curl;
mod detail_tabs;
mod detail_view;
mod filter;
mod fonts;
mod har;
mod har_dialog;
mod image_decode;
mod image_view;
mod outcome_view;
mod port_settings;
mod range_merge;
mod records;
mod replay_panel;
mod request_table;
mod save;
mod scope_help;
mod search;
mod theme;
mod toolbar;
mod tree_view;

use self::detail_tabs::DetailTab;
use self::filter::RecordFilter;
use self::toolbar::PortChoice;

use std::path::PathBuf;
use std::time::Duration;

use eframe::egui;
use http_proxy::{
    EngineState, Exchange, FullLog, HttpsInspection, ProxyConfig, ProxyHandle, RecordId,
};

const MAX_RECORDS: usize = 2_000;
/// Repaint interval while pending requests or streaming bodies change on screen.
const LIVE_REPAINT: Duration = Duration::from_millis(250);
/// Repaint interval otherwise, to notice an engine that stopped on its own.
const IDLE_REPAINT: Duration = Duration::from_secs(1);
/// Default window size, large enough for the list and details side by side.
const DEFAULT_WINDOW: [f32; 2] = [1_280.0, 820.0];
const MIN_WINDOW: [f32; 2] = [640.0, 420.0];
/// Height kept for the request list and details even in a short window,
/// about ten table rows; the window scrolls instead of shrinking it further.
const MIN_MAIN_HEIGHT: f32 = 280.0;
const MIN_TABLE_WIDTH: f32 = 320.0;
const MIN_DETAIL_WIDTH: f32 = 280.0;
/// Share of the main area the details panel takes before the user drags it.
const DEFAULT_DETAIL_SHARE: f32 = 0.4;

pub fn main() -> eframe::Result {
    init_tracing();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size(DEFAULT_WINDOW)
            .with_min_inner_size(MIN_WINDOW),
        ..eframe::NativeOptions::default()
    };
    eframe::run_native(
        http_proxy::APP_NAME,
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

struct InspectorApp {
    theme: theme::ThemeSettings,
    replay: Option<replay_panel::ReplayPanel>,
    curl: copy_curl::CurlCopy,
    proxy: Option<ProxyHandle>,
    records: records::RecordList<Exchange>,
    selected: Option<RecordId>,
    search: search::RequestSearch,
    record_filter: RecordFilter,
    https_inspection: bool,
    authorization_notice: toolbar::AuthorizationNotice,
    ca_directory: Option<PathBuf>,
    ca_panel: ca_panel::CaPanel,
    browsers: browser_panel::BrowserPanel,
    error: Option<String>,
    full_log: Option<FullLog>,
    log_init_error: Option<String>,
    save: save::SaveControls,
    har_dialog: har_dialog::HarDialog,
    body_saving: body_saving::BodySaving,
    port: PortChoice,
    table: request_table::RequestTable,
    detail_tab: DetailTab,
    body_mode: body_format::ViewMode,
    body_cache: body_display::BodyViewCache,
    image: image_view::ImagePanel,
    /// Height of the status bar in the previous frame, reserved below the
    /// main area so that it fits in the window without scrolling.
    status_bar_height: f32,
    /// Fraction of the main area given to the details pane before clamping.
    /// Updated only when the user drags the divider, so shrinking the window
    /// and widening it again restores the split instead of staying narrow.
    detail_share: f32,
    context: egui::Context,
}

impl InspectorApp {
    fn new(context: egui::Context) -> Self {
        let theme = theme::ThemeSettings::new(&context);
        let (full_log, log_init_error) = match FullLog::new() {
            Ok(log) => (Some(log), None),
            Err(error) => (
                None,
                Some(format!("Full-session logging is unavailable: {error}")),
            ),
        };
        let mut app = Self {
            replay: None,
            curl: copy_curl::CurlCopy::default(),
            proxy: None,
            records: records::RecordList::new(MAX_RECORDS),
            selected: None,
            search: search::RequestSearch::new(&context),
            record_filter: RecordFilter::All,
            https_inspection: false,
            authorization_notice: toolbar::AuthorizationNotice::default(),
            ca_directory: app_paths::ca_directory(|name| std::env::var_os(name)),
            ca_panel: ca_panel::CaPanel::default(),
            browsers: browser_panel::BrowserPanel::default(),
            error: None,
            full_log,
            log_init_error,
            save: save::SaveControls::new(),
            har_dialog: har_dialog::HarDialog::default(),
            body_saving: body_saving::BodySaving::open(),
            port: PortChoice::from_launch(),
            table: request_table::RequestTable::new(),
            detail_tab: DetailTab::Headers,
            body_mode: body_format::ViewMode::default(),
            body_cache: body_display::BodyViewCache::default(),
            image: image_view::ImagePanel::default(),
            status_bar_height: 0.0,
            detail_share: DEFAULT_DETAIL_SHARE,
            theme,
            context,
        };
        app.start_proxy();
        app
    }

    fn start_proxy(&mut self) {
        let port = match port_settings::ProxyPort::parse(&self.port.text) {
            Ok(port) => port,
            Err(error) => {
                self.error = Some(error.to_string());
                return;
            }
        };
        let mut config = ProxyConfig::loopback(port.value());
        if self.https_inspection {
            let Some(directory) = self.ca_directory.as_ref() else {
                self.error = Some(
                    "HTTPS inspection requires an application data directory; check HOME and platform storage settings"
                        .to_owned(),
                );
                return;
            };
            config = config.with_https_inspection(HttpsInspection::new(directory.clone()));
        }
        if let Some(store) = self.body_saving.store() {
            config = config.with_body_store(store.clone());
        }
        let result = self.full_log.as_ref().map_or_else(
            || ProxyHandle::start(config.clone()),
            |log| ProxyHandle::start_recording(config.clone(), log.clone()),
        );
        match result {
            Ok(proxy) => {
                let context = self.context.clone();
                let _first_notifier = proxy.notify_on_event(move || context.request_repaint());
                self.proxy = Some(proxy);
                self.error = self
                    .port
                    .remember(port)
                    .err()
                    .map(|error| error.to_string());
            }
            Err(error) => {
                self.error = Some(port_settings::start_error_text(&error, port));
            }
        }
    }

    fn stop_proxy(&mut self) {
        self.browsers.close();
        self.ca_panel.refresh();
        if let Some(proxy) = self.take_proxy()
            && let Err(error) = proxy.stop()
        {
            self.error = Some(error.to_string());
        }
    }

    /// Releases a proxy whose engine stopped on its own so the toolbar shows
    /// `Stopped` with the cause and `Start` can be pressed again.
    fn release_failed_proxy(&mut self) {
        let Some(EngineState::Failed { message }) = self.proxy.as_ref().map(ProxyHandle::state)
        else {
            return;
        };
        self.receive_records();
        let mut error = format!("The proxy stopped: {message}. Press Start to restart it.");
        if let Some(proxy) = self.take_proxy()
            && let Err(stop_error) = proxy.stop()
        {
            error = format!("{error} ({stop_error})");
        }
        self.error = Some(error);
    }

    /// Detaches the running proxy, keeping its drop count for the toolbar.
    fn take_proxy(&mut self) -> Option<ProxyHandle> {
        let proxy = self.proxy.take()?;
        self.records.proxy_stopped(proxy.dropped_records());
        Some(proxy)
    }

    fn engine_dropped(&self) -> u64 {
        self.proxy.as_ref().map_or(0, ProxyHandle::dropped_records)
    }

    fn receive_records(&mut self) {
        let Some(proxy) = self.proxy.as_mut() else {
            return;
        };
        while let Some(record) = proxy.try_receive() {
            if let Some(removed) = self.records.push(record)
                && self.selected == Some(removed.id())
            {
                self.selected = None;
            }
        }
    }

    /// Reports whether any displayed value still changes over time.
    fn has_live_records(&self) -> bool {
        self.records.iter().any(Exchange::is_live)
    }
}

impl eframe::App for InspectorApp {
    fn clear_color(&self, visuals: &egui::Visuals) -> [f32; 4] {
        visuals.panel_fill.to_normalized_gamma_f32()
    }

    fn ui(&mut self, ui: &mut eframe::egui::Ui, _frame: &mut eframe::Frame) {
        self.render_ui(ui);
    }
}

impl InspectorApp {
    /// Renders one frame of the inspector inside `ui`.
    pub(in crate::ui) fn render_ui(&mut self, ui: &mut egui::Ui) {
        self.receive_records();
        self.release_failed_proxy();
        self.body_saving.update(ui.ctx());
        if let Some(panel) = self.replay.as_mut() {
            let latest = self
                .records
                .iter()
                .rev()
                .find(|record| record.replay_of() == Some(panel.original_id()));
            if !panel.show(ui.ctx(), self.proxy.as_ref(), latest) {
                self.replay = None;
            }
        }
        self.har_export(ui.ctx());
        self.bulk_body_save(ui.ctx());
        self.range_merge(ui.ctx());
        let shortcuts_allowed = request_shortcuts_allowed(ui.ctx());
        let viewport_height = ui.available_height();
        egui::ScrollArea::vertical()
            .id_salt("window-scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let top = ui.cursor().top();
                self.toolbar(ui);
                self.notices(ui);
                ui.separator();
                self.filters(ui);
                let used = ui.cursor().top() - top + ui.spacing().item_spacing.y;
                let height = main_height(viewport_height, used, self.status_bar_height);
                self.main_area(ui, height, shortcuts_allowed);
                let status_top = ui.cursor().top();
                ui.separator();
                self.status_bar(ui);
                self.status_bar_height = ui.min_rect().bottom() - status_top;
            });

        // New exchanges wake the UI through the proxy's notifier. Periodic
        // repaints are needed only for values that change without an event
        // and for noticing an engine that stopped on its own.
        if self.has_live_records() {
            ui.ctx().request_repaint_after(LIVE_REPAINT);
        } else if self.proxy.is_some() {
            ui.ctx().request_repaint_after(IDLE_REPAINT);
        }
    }

    /// Shows the `Export HAR` window and starts the export it confirms.
    fn har_export(&mut self, context: &egui::Context) {
        if !self.har_dialog.is_open() {
            return;
        }
        let counts = har_dialog::ScopeCounts {
            all: self.records.len(),
            filtered: self.filtered_records().len(),
        };
        let Some(export) = self.har_dialog.show(context, counts, self.save.is_busy()) else {
            return;
        };
        let records: Vec<Exchange> = match export.scope {
            har_dialog::HarScope::All => self.records.iter().cloned().collect(),
            har_dialog::HarScope::Filtered => {
                self.filtered_records().into_iter().cloned().collect()
            }
        };
        self.save
            .export_har(context, records, export.options, export.path);
    }

    fn filtered_records(&self) -> Vec<&Exchange> {
        filter::visible_records(&self.records, self.record_filter, &self.search)
    }

    /// Requests on the left and details on the right, split by a draggable
    /// divider. Each pane is strictly clipped to its own disjoint rectangle
    /// so wide content in either pane can never paint over the other.
    fn main_area(&mut self, ui: &mut egui::Ui, height: f32, shortcuts_allowed: bool) {
        const SPLIT_HANDLE_HALF_WIDTH: f32 = 3.0;
        const DETAIL_LEFT_MARGIN: f32 = 8.0;

        let size = egui::vec2(ui.available_width(), height);
        let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
        let mut main = ui.new_child(
            egui::UiBuilder::new()
                .id_salt("main-area")
                .max_rect(rect)
                .layout(egui::Layout::top_down(egui::Align::Min)),
        );
        main.shrink_clip_rect(rect);

        let width = rect.width();
        let split_id = main.make_persistent_id("main-split");
        if let Some(prev) = main.ctx().read_response(split_id)
            && (prev.dragged() || prev.drag_stopped())
            && let Some(pointer) = prev.interact_pointer_pos()
            && width > 0.0
        {
            let (min_detail, max_detail) = detail_width_bounds(width);
            let dragged = (rect.right() - pointer.x).clamp(min_detail, max_detail);
            self.detail_share = (dragged / width).clamp(0.0, 1.0);
        }

        let detail_width = split_detail_width(width, self.detail_share);
        let split_x = (rect.right() - detail_width)
            .round()
            .clamp(rect.left(), rect.right());
        let table_rect = egui::Rect::from_min_max(
            rect.min,
            egui::pos2((split_x - 1.0).max(rect.left()), rect.max.y),
        );
        let detail_outer = egui::Rect::from_min_max(
            egui::pos2((split_x + 1.0).min(rect.right()), rect.min.y),
            rect.max,
        );
        let detail_rect = egui::Rect::from_min_max(
            egui::pos2(
                (detail_outer.left() + DETAIL_LEFT_MARGIN).min(detail_outer.right()),
                detail_outer.top(),
            ),
            detail_outer.max,
        );

        let mut table_ui = main.new_child(
            egui::UiBuilder::new()
                .id_salt("requests-pane")
                .max_rect(table_rect)
                .layout(egui::Layout::top_down(egui::Align::Min)),
        );
        table_ui.shrink_clip_rect(table_rect);
        self.request_table(&mut table_ui, shortcuts_allowed);

        let mut detail_ui = main.new_child(
            egui::UiBuilder::new()
                .id_salt("details-pane")
                .max_rect(detail_rect)
                .layout(egui::Layout::top_down(egui::Align::Min)),
        );
        detail_ui.shrink_clip_rect(detail_rect);
        self.detail_panel(&mut detail_ui, shortcuts_allowed);

        let handle_rect = egui::Rect::from_x_y_ranges(
            (split_x - SPLIT_HANDLE_HALF_WIDTH).max(rect.left())
                ..=(split_x + SPLIT_HANDLE_HALF_WIDTH).min(rect.right()),
            rect.y_range(),
        );
        let handle = main.interact(handle_rect, split_id, egui::Sense::drag());
        if handle.dragged() || handle.drag_stopped() {
            main.ctx().request_repaint();
        }
        if handle.hovered() || handle.dragged() {
            main.ctx()
                .set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
        }
        let stroke = if handle.dragged() {
            main.visuals().widgets.active.fg_stroke
        } else if handle.hovered() {
            main.visuals().widgets.hovered.fg_stroke
        } else {
            main.visuals().widgets.noninteractive.bg_stroke
        };
        main.painter().vline(split_x, rect.y_range(), stroke);
    }
}

/// Minimum and maximum width of the right details pane for `total_width`.
fn detail_width_bounds(total_width: f32) -> (f32, f32) {
    let min_detail = MIN_DETAIL_WIDTH.min((total_width * 0.5).floor());
    let max_detail = (total_width - MIN_TABLE_WIDTH).max(min_detail);
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
    (viewport - above - below).floor().max(MIN_MAIN_HEIGHT)
}

fn request_shortcuts_allowed(ctx: &egui::Context) -> bool {
    let focused = ctx.memory(egui::Memory::focused);
    !egui::Popup::is_any_open(ctx)
        && focused.is_none_or(|id| egui::TextEdit::load_state(ctx, id).is_none())
}

#[cfg(test)]
mod layout_tests {
    use super::{
        DEFAULT_DETAIL_SHARE, DEFAULT_WINDOW, DetailTab, InspectorApp, MAX_RECORDS,
        MIN_DETAIL_WIDTH, MIN_MAIN_HEIGHT, MIN_TABLE_WIDTH, MIN_WINDOW, PortChoice, RecordFilter,
        body_display, body_format, body_saving, browser_panel, ca_panel, copy_curl, fonts,
        har_dialog, image_view, main_height, port_settings, records, replay_panel, request_table,
        save, search, split_detail_width, theme, toolbar,
    };
    use eframe::egui;
    use http_proxy::{Exchange, ProxyConfig, ProxyHandle};
    use std::time::Duration;
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    use tokio::net::{TcpListener, TcpStream};
    use tokio::time::timeout;

    fn test_app(context: egui::Context) -> InspectorApp {
        fonts::configure_fonts(&context);
        InspectorApp {
            theme: theme::ThemeSettings::new(&context),
            replay: None,
            curl: copy_curl::CurlCopy::default(),
            proxy: None,
            records: records::RecordList::new(MAX_RECORDS),
            selected: None,
            search: search::RequestSearch::new(&context),
            record_filter: RecordFilter::All,
            https_inspection: false,
            authorization_notice: toolbar::AuthorizationNotice::default(),
            ca_directory: None,
            ca_panel: ca_panel::CaPanel::default(),
            browsers: browser_panel::BrowserPanel::default(),
            error: None,
            full_log: None,
            log_init_error: None,
            save: save::SaveControls::new(),
            har_dialog: har_dialog::HarDialog::default(),
            body_saving: body_saving::BodySaving::without_store(),
            port: PortChoice {
                text: "0".to_owned(),
                origin: port_settings::PortOrigin::Default,
                settings_directory: None,
                launch_warning: None,
            },
            table: request_table::RequestTable::new(),
            detail_tab: DetailTab::Headers,
            body_mode: body_format::ViewMode::Pretty,
            body_cache: body_display::BodyViewCache::default(),
            image: image_view::ImagePanel::default(),
            status_bar_height: 0.0,
            detail_share: DEFAULT_DETAIL_SHARE,
            context,
        }
    }

    #[test]
    fn main_area_fills_the_rest_but_keeps_a_minimum() {
        assert!((main_height(700.0, 90.0, 30.0) - 580.0).abs() < f32::EPSILON);
        assert!((main_height(300.0, 90.0, 30.0) - MIN_MAIN_HEIGHT).abs() < f32::EPSILON);
    }

    #[test]
    fn split_detail_width_clamps_narrow_and_restores_wide() {
        let narrow_total = MIN_WINDOW[0] - 16.0;
        let narrow_detail = split_detail_width(narrow_total, DEFAULT_DETAIL_SHARE);
        assert!(narrow_detail >= MIN_DETAIL_WIDTH);
        assert!(narrow_total - narrow_detail >= MIN_TABLE_WIDTH);

        // Widening again from the same stored share restores proportional detail space.
        let wide_total = DEFAULT_WINDOW[0] - 16.0;
        let wide_detail = split_detail_width(wide_total, DEFAULT_DETAIL_SHARE);
        assert!(wide_detail > narrow_detail + 150.0);
        assert!(wide_total.mul_add(-DEFAULT_DETAIL_SHARE, wide_detail).abs() < 1.0);

        // A user-dragged custom share is clamped when narrow and recovered when wide.
        let custom_narrow = split_detail_width(narrow_total, 0.55);
        assert!((custom_narrow - (narrow_total - MIN_TABLE_WIDTH)).abs() < 1.0);
        let custom_wide = split_detail_width(wide_total, 0.55);
        assert!(wide_total.mul_add(-0.55, custom_wide).abs() < 1.0);
    }

    struct DrawnText {
        text: String,
        rect: egui::Rect,
    }

    fn collect_text_shapes(shapes: &[egui::epaint::ClippedShape]) -> Vec<DrawnText> {
        let mut drawn = Vec::new();
        for clipped in shapes {
            let clip = clipped.clip_rect;
            if !clip.is_positive() {
                continue;
            }
            push_shape_text(&clipped.shape, clip, &mut drawn);
        }
        drawn
    }

    fn push_shape_text(shape: &egui::Shape, clip: egui::Rect, out: &mut Vec<DrawnText>) {
        match shape {
            egui::Shape::Text(text) => {
                let raw = text.galley.text().trim();
                if raw.is_empty() {
                    return;
                }
                let rect = text.visual_bounding_rect().intersect(clip);
                if rect.width() > 1.0 && rect.height() > 1.0 {
                    out.push(DrawnText {
                        text: raw.to_owned(),
                        rect,
                    });
                }
            }
            egui::Shape::Vec(items) => {
                for item in items {
                    push_shape_text(item, clip, out);
                }
            }
            _ => {}
        }
    }

    fn assert_no_text_overlap(drawn: &[DrawnText], viewport: egui::Rect, label: &str) {
        const TOLERANCE: f32 = 1.5;
        for item in drawn {
            assert!(
                item.rect.left() >= viewport.left() - TOLERANCE
                    && item.rect.right() <= viewport.right() + TOLERANCE
                    && item.rect.top() >= viewport.top() - TOLERANCE
                    && item.rect.bottom() <= viewport.bottom() + TOLERANCE,
                "{label}: text {:?} ({:?}) overflowed viewport {:?}",
                item.text,
                item.rect,
                viewport,
            );
        }
        for (i, first) in drawn.iter().enumerate() {
            for second in drawn.iter().skip(i + 1) {
                let overlap = first.rect.intersect(second.rect);
                assert!(
                    overlap.width() <= TOLERANCE || overlap.height() <= TOLERANCE,
                    "{label}: overlapping text {:?} ({:?}) and {:?} ({:?}), overlap={:?}",
                    first.text,
                    first.rect,
                    second.text,
                    second.rect,
                    overlap,
                );
            }
        }
    }

    fn render_frame(
        app: &mut InspectorApp,
        size: [f32; 2],
        events: Vec<egui::Event>,
    ) -> (Vec<DrawnText>, Duration) {
        let viewport = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(size[0], size[1]));
        let input = egui::RawInput {
            screen_rect: Some(viewport),
            events,
            ..Default::default()
        };
        let mut output = app.context.clone().run_ui(input, |ui| {
            egui::CentralPanel::default().show(ui, |ui| app.render_ui(ui));
        });
        output.textures_delta.clear();
        let repaint_delay = output
            .viewport_output
            .get(&egui::ViewportId::ROOT)
            .map_or(Duration::MAX, |viewport_out| viewport_out.repaint_delay);
        (collect_text_shapes(&output.shapes), repaint_delay)
    }

    fn render_settled(app: &mut InspectorApp, size: [f32; 2]) -> Vec<DrawnText> {
        let mut drawn = Vec::new();
        for _ in 0..4 {
            let (frame_drawn, _) = render_frame(app, size, Vec::new());
            drawn = frame_drawn;
        }
        drawn
    }

    async fn read_head(stream: &mut TcpStream) -> std::io::Result<()> {
        let mut head = Vec::new();
        let mut byte = [0; 1];
        while !head.ends_with(b"\r\n\r\n") {
            stream.read_exact(&mut byte).await?;
            head.extend_from_slice(&byte);
        }
        Ok(())
    }

    async fn capture_sample_exchanges() -> Result<Vec<Exchange>, Box<dyn std::error::Error>> {
        let upstream = TcpListener::bind("127.0.0.1:0").await?;
        let address = upstream.local_addr()?;
        let mut proxy = ProxyHandle::start(ProxyConfig::loopback(0))?;
        let mut captured = Vec::new();

        let cases: [(&str, &str, &str); 3] = [
            (
                "OPTIONS",
                "/api/v2/users",
                "HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            ),
            (
                "POST",
                "/api/v2/orders",
                "HTTP/1.1 201 Created\r\nContent-Type: application/json\r\nContent-Length: 18\r\nConnection: close\r\n\r\n{\"created\":true}\r\n",
            ),
            (
                "GET",
                "/api/v2/users?page=7&filter=active&sort=created_at_desc&token=abcdef0123456789",
                "HTTP/1.1 200 OK\r\nContent-Type: application/json; charset=utf-8\r\nX-Request-Id: req-20261123-abcdef0123456789-extra-long-value\r\nX-Custom-Very-Long-Response-Header-Name: value-spanning-more-than-narrow-detail-pane\r\nCache-Control: no-store, max-age=0\r\nContent-Length: 59\r\nConnection: close\r\n\r\n{\"users\":[{\"id\":1,\"name\":\"Alice\"},{\"id\":2,\"name\":\"Bob\"}]}\r\n",
            ),
        ];

        for (method, path, response) in cases {
            let mut client = TcpStream::connect(proxy.local_addr()).await?;
            let request = format!(
                "{method} http://{address}{path} HTTP/1.1\r\nHost: {address}\r\nX-Custom-Very-Long-Request-Header-Name: long-request-header-value-0123456789abcdef\r\nConnection: close\r\n\r\n"
            );
            client.write_all(request.as_bytes()).await?;
            let (mut server, _) = upstream.accept().await?;
            let record = proxy.receive().await.ok_or("missing captured exchange")?;
            read_head(&mut server).await?;
            server.write_all(response.as_bytes()).await?;
            drop(server);
            let mut sink = Vec::new();
            client.read_to_end(&mut sink).await?;
            while record.outcome().is_in_flight() {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
            captured.push(record);
        }

        proxy.stop()?;
        Ok(captured)
    }

    #[tokio::test]
    async fn narrow_and_resized_windows_do_not_overlap_text()
    -> Result<(), Box<dyn std::error::Error>> {
        let exchanges = timeout(Duration::from_secs(5), capture_sample_exchanges()).await??;
        let ctx = egui::Context::default();
        let mut app = test_app(ctx);
        for exchange in exchanges {
            app.selected = Some(exchange.id());
            let _evicted = app.records.push(exchange);
        }

        for size in [
            DEFAULT_WINDOW,
            [800.0, 420.0],
            [700.0, 420.0],
            MIN_WINDOW,
            [MIN_WINDOW[0], 900.0],
            [1_100.0, 700.0],
        ] {
            app.detail_tab = DetailTab::Headers;
            let viewport =
                egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(size[0], size[1]));
            let drawn = render_settled(&mut app, size);
            assert!(!drawn.is_empty(), "expected painted text at {size:?}");
            assert_no_text_overlap(&drawn, viewport, &format!("headers@{size:?}"));
            assert!(
                drawn.iter().any(|item| item.text == "OPTIONS"),
                "expected untruncated OPTIONS method at {size:?}, got methods: {:?}",
                drawn
                    .iter()
                    .filter(|item| item.text.starts_with("OPT"))
                    .map(|item| &item.text)
                    .collect::<Vec<_>>(),
            );

            // At 640x900, query parameters and wrapped request/response headers are all visible.
            if (size[0] - MIN_WINDOW[0]).abs() < 1.0 && (size[1] - 900.0).abs() < 1.0 {
                assert!(
                    drawn.iter().any(|item| item.text.contains("x-request-id")),
                    "expected wrapped response headers to be visible at 640x900"
                );
            }

            // At 1100x700 after shrinking to 640px, verify the details pane recovered its width.
            if (size[0] - 1_100.0).abs() < 1.0 {
                let edit_button = drawn
                    .iter()
                    .find(|item| item.text == "Edit and Resend")
                    .ok_or("missing Edit and Resend button at 1100x700")?;
                // 40% detail share on 1100px puts details starting around x ≈ 650,
                // well left of the cramped ~780px position a stuck narrow panel would keep.
                assert!(
                    edit_button.rect.left() < 760.0,
                    "details pane did not recover width on enlarge: {:#?}",
                    edit_button.rect
                );
            }
        }

        for mode in body_format::ViewMode::ALL {
            app.detail_tab = DetailTab::Body;
            app.body_mode = mode;
            let viewport = egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(MIN_WINDOW[0], MIN_WINDOW[1]),
            );
            let drawn = render_settled(&mut app, MIN_WINDOW);
            assert_no_text_overlap(&drawn, viewport, &format!("body-{mode:?}@640x420"));
        }

        // Empty list and unselected detail pane at minimum window size.
        app.records.clear(0);
        app.selected = None;
        let min_viewport =
            egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(MIN_WINDOW[0], MIN_WINDOW[1]));
        let drawn = render_settled(&mut app, MIN_WINDOW);
        assert_no_text_overlap(&drawn, min_viewport, "empty@640x420");

        Ok(())
    }

    fn drag_pointer(
        app: &mut InspectorApp,
        size: [f32; 2],
        from: egui::Pos2,
        to: egui::Pos2,
    ) -> (Duration, Duration) {
        let _ = render_frame(app, size, vec![egui::Event::PointerMoved(from)]);
        let _ = render_frame(
            app,
            size,
            vec![egui::Event::PointerButton {
                pos: from,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            }],
        );
        let (_, drag_delay) = render_frame(app, size, vec![egui::Event::PointerMoved(to)]);
        let (_, release_delay) = render_frame(
            app,
            size,
            vec![egui::Event::PointerButton {
                pos: to,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }],
        );
        (drag_delay, release_delay)
    }

    fn text_left(drawn: &[DrawnText], label: &str) -> Result<f32, String> {
        drawn
            .iter()
            .find(|item| item.text == label)
            .map(|item| item.rect.left())
            .ok_or_else(|| format!("missing {label:?} text"))
    }

    #[tokio::test]
    async fn split_divider_drag_repaints_and_restores_across_resize()
    -> Result<(), Box<dyn std::error::Error>> {
        let exchanges = timeout(Duration::from_secs(5), capture_sample_exchanges()).await??;
        let mut app = test_app(egui::Context::default());
        for exchange in exchanges {
            app.selected = Some(exchange.id());
            let _evicted = app.records.push(exchange);
        }

        let wide = [1_100.0, 700.0];
        let initial_edit_left = text_left(&render_settled(&mut app, wide), "Edit and Resend")?;

        // CentralPanel defaults to 8px horizontal margin on each side.
        let main_width = wide[0] - 16.0;
        let start_x = (8.0 + main_width - split_detail_width(main_width, app.detail_share)).round();
        let (drag_delay, release_delay) = drag_pointer(
            &mut app,
            wide,
            egui::pos2(start_x, 360.0),
            egui::pos2(start_x - 140.0, 360.0),
        );
        assert_eq!(
            drag_delay,
            Duration::ZERO,
            "dragging the split divider must request an immediate repaint"
        );
        assert_eq!(
            release_delay,
            Duration::ZERO,
            "releasing the split divider must request an immediate repaint for the final position"
        );

        let (after_drag, _) = render_frame(&mut app, wide, Vec::new());
        assert!(
            app.detail_share > 0.50,
            "expected dragged detail_share > 0.50, got {}",
            app.detail_share
        );
        let dragged_share = app.detail_share;
        let dragged_edit_left = text_left(&after_drag, "Edit and Resend")?;
        assert!(
            dragged_edit_left < initial_edit_left - 100.0,
            "details pane should widen immediately after drag release: before={initial_edit_left}, after={dragged_edit_left}"
        );

        // Shrink to minimum size: clamped cleanly without overwriting the user's preferred share.
        let min_viewport =
            egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(MIN_WINDOW[0], MIN_WINDOW[1]));
        let narrow_drawn = render_settled(&mut app, MIN_WINDOW);
        assert_no_text_overlap(&narrow_drawn, min_viewport, "dragged-shrunk@640x420");
        assert!((app.detail_share - dragged_share).abs() < f32::EPSILON);

        // Enlarge back to wide: user-dragged split width is restored.
        let wide_viewport =
            egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(wide[0], wide[1]));
        let restored_drawn = render_settled(&mut app, wide);
        assert_no_text_overlap(&restored_drawn, wide_viewport, "dragged-restored@1100x700");
        let restored_edit_left = text_left(&restored_drawn, "Edit and Resend")?;
        assert!(
            (restored_edit_left - dragged_edit_left).abs() < 4.0,
            "expected restored detail position near {dragged_edit_left}, got {restored_edit_left}"
        );

        Ok(())
    }

    #[tokio::test]
    async fn dialogs_fit_minimum_window_without_text_overlap()
    -> Result<(), Box<dyn std::error::Error>> {
        let exchanges = timeout(Duration::from_secs(5), capture_sample_exchanges()).await??;
        let sample = exchanges.last().cloned().ok_or("missing sample exchange")?;
        let viewport =
            egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(MIN_WINDOW[0], MIN_WINDOW[1]));

        // Export HAR dialog at 640x420.
        let ctx = egui::Context::default();
        fonts::configure_fonts(&ctx);
        let mut har = har_dialog::HarDialog::default();
        har.open("/tmp/example-export-path/session-20261123.har".to_owned());
        let counts = har_dialog::ScopeCounts {
            all: 12,
            filtered: 6,
        };
        let mut drawn = Vec::new();
        for _ in 0..3 {
            let input = egui::RawInput {
                screen_rect: Some(viewport),
                ..Default::default()
            };
            let mut output = ctx.run_ui(input, |ui| {
                let _ = har.show(ui.ctx(), counts, false);
            });
            output.textures_delta.clear();
            drawn = collect_text_shapes(&output.shapes);
        }
        assert_no_text_overlap(&drawn, viewport, "har-dialog@640x420");
        assert!(drawn.iter().any(|item| item.text == "Cancel"));

        // Replay editor at 640x420.
        let ctx = egui::Context::default();
        fonts::configure_fonts(&ctx);
        let mut replay = replay_panel::ReplayPanel::new(sample)?;
        let mut drawn = Vec::new();
        for _ in 0..3 {
            let input = egui::RawInput {
                screen_rect: Some(viewport),
                ..Default::default()
            };
            let mut output = ctx.run_ui(input, |ui| {
                let _ = replay.show(ui.ctx(), None, None);
            });
            output.textures_delta.clear();
            drawn = collect_text_shapes(&output.shapes);
        }
        assert_no_text_overlap(&drawn, viewport, "replay-panel@640x420");

        // Bulk body save modal at 640x420.
        let ctx = egui::Context::default();
        let mut app = test_app(ctx);
        app.body_saving.open_bulk();
        let mut drawn = Vec::new();
        for _ in 0..3 {
            let input = egui::RawInput {
                screen_rect: Some(viewport),
                ..Default::default()
            };
            let mut output = app.context.clone().run_ui(input, |ui| {
                app.bulk_body_save(ui.ctx());
            });
            output.textures_delta.clear();
            drawn = collect_text_shapes(&output.shapes);
        }
        assert_no_text_overlap(&drawn, viewport, "bulk-save@640x420");
        assert!(drawn.iter().any(|item| item.text == "Save"));
        assert!(drawn.iter().any(|item| item.text == "Cancel"));

        Ok(())
    }
}
