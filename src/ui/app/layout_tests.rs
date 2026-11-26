use super::{
    AnalysisPipeline, DetailState, Dialogs, InspectorApp, Layout, LayoutState, MAX_RECORDS,
    MainView, MapState, RequestsState, SessionState, fonts, main_height, split_detail_width,
};
use crate::ui::detail::detail_tabs::DetailTab;
use crate::ui::dialogs::{body_saving::BodySaving, har_dialog, replay_panel};
use eframe::egui;
use http_inspector_analysis::url_map::UrlMap;
use http_inspector_content::format as body_format;
use http_inspector_core::interception::{ProxyConfig, ProxyHandle};
use http_inspector_core::recording::PreviewBodies;
use http_inspector_exchange::Exchange;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::{TcpListener, TcpStream};
use tokio::time::timeout;

fn test_app(context: egui::Context) -> InspectorApp {
    fonts::configure_fonts(&context);
    let mut layout = LayoutState::new(&context);
    layout.language = crate::ui::i18n::LanguageSettings::fixed(crate::ui::i18n::Language::English);
    InspectorApp {
        session: SessionState {
            capture: super::capture::CaptureSession::fixture(),
            body_saving: BodySaving::without_store(),
            error: None,
        },
        requests: RequestsState::new(&context),
        request_table: super::RequestTable::new(),
        detail: DetailState::default(),
        map: MapState::new(AnalysisPipeline::disabled()),
        dialogs: Dialogs::new(),
        layout,
        context,
    }
}

#[test]
fn clear_drops_capture_state_and_keeps_view_preferences() -> Result<(), Box<dyn std::error::Error>>
{
    use http::Method;
    use http_inspector_analysis::filter::RecordFilter;
    use http_inspector_analysis::url::OriginKey;
    use http_inspector_analysis::url_map::{RequestScope, ViewNodeKey};
    use http_inspector_exchange::ExchangeBuilder;

    let mut app = test_app(egui::Context::default());
    let record = ExchangeBuilder::http(Method::GET, "http://fixture.test/api")?.in_flight()?;
    let key = ViewNodeKey {
        origin: OriginKey::new("http", "fixture.test", None),
        path: Vec::new(),
    };
    app.map.analysis.record_added(&record);
    app.map.panel.selected = Some(key.clone());
    app.map.panel.sub_view = super::MapSubView::Graph;
    let id = record.id();
    let _evicted = super::capture::add_record(
        &mut app.requests.records,
        &mut app.requests.selected,
        record,
    );
    app.requests.show_map_scope(RequestScope::subtree(key));
    app.requests.selected = Some(id);
    app.requests.record_filter = RecordFilter::Failures;
    app.detail.tab = DetailTab::Body;
    app.detail.body_mode = body_format::ViewMode::Raw;
    app.dialogs.har.open("fixture.har".to_owned());
    app.session.capture.port.text = "8081".to_owned();
    app.layout.main_view = MainView::Map;
    app.layout.detail_share = 0.52;
    assert!(app.map.analysis.url_map().node_count() > 0);
    assert_eq!(app.requests.map_filter_cache.matched_count(), 1);

    app.clear_session();

    assert!(app.requests.records.is_empty());
    assert_eq!(app.requests.selected, None);
    assert_eq!(app.requests.records.evicted(), 0);
    assert!(app.requests.map_path_filter.is_none());
    assert_eq!(app.requests.map_filter_cache.matched_count(), 0);
    assert_eq!(app.map.analysis.url_map().node_count(), 0);
    assert!(app.map.panel.selected.is_none());

    assert_eq!(app.requests.record_filter, RecordFilter::Failures);
    assert_eq!(app.map.panel.sub_view, super::MapSubView::Graph);
    assert_eq!(app.detail.tab, DetailTab::Body);
    assert_eq!(app.detail.body_mode, body_format::ViewMode::Raw);
    assert!(app.dialogs.har.is_open());
    assert_eq!(app.session.capture.port.text, "8081");
    assert_eq!(app.layout.main_view, MainView::Map);
    assert!((app.layout.detail_share - 0.52).abs() < f32::EPSILON);
    Ok(())
}

#[test]
fn invalid_port_leaves_proxy_stopped_with_a_toolbar_error() {
    let mut app = test_app(egui::Context::default());
    app.session.capture.port.text.clear();

    app.start_proxy();

    assert!(app.session.capture.proxy.is_none());
    assert_eq!(
        app.session
            .capture
            .error
            .as_ref()
            .map(|notice| {
                crate::ui::requests::toolbar::notice_text(
                    notice,
                    crate::ui::i18n::Language::English,
                )
            })
            .as_deref(),
        Some("Enter a port number, or 0 to pick a free port.")
    );
}

#[test]
fn main_area_fills_the_rest_but_keeps_a_minimum() {
    assert!((main_height(700.0, 90.0, 30.0) - 580.0).abs() < f32::EPSILON);
    assert!((main_height(300.0, 90.0, 30.0) - Layout::MIN_MAIN_HEIGHT).abs() < f32::EPSILON);
}

#[test]
fn split_detail_width_clamps_narrow_and_restores_wide() {
    let narrow_total = Layout::MIN_WINDOW[0] - 16.0;
    let narrow_detail = split_detail_width(narrow_total, Layout::DEFAULT_DETAIL_SHARE);
    assert!(narrow_detail >= Layout::MIN_DETAIL_WIDTH);
    assert!(narrow_total - narrow_detail >= Layout::MIN_TABLE_WIDTH);

    // Widening again from the same stored share restores proportional detail space.
    let wide_total = Layout::WINDOW[0] - 16.0;
    let wide_detail = split_detail_width(wide_total, Layout::DEFAULT_DETAIL_SHARE);
    assert!(wide_detail > narrow_detail + 150.0);
    assert!(
        wide_total
            .mul_add(-Layout::DEFAULT_DETAIL_SHARE, wide_detail)
            .abs()
            < 1.0
    );

    // A user-dragged custom share is clamped when narrow and recovered when wide.
    let custom_narrow = split_detail_width(narrow_total, 0.55);
    assert!((custom_narrow - (narrow_total - Layout::MIN_TABLE_WIDTH)).abs() < 1.0);
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
async fn narrow_and_resized_windows_do_not_overlap_text() -> Result<(), Box<dyn std::error::Error>>
{
    let exchanges = timeout(Duration::from_secs(5), capture_sample_exchanges()).await??;
    let ctx = egui::Context::default();
    let mut app = test_app(ctx);
    for exchange in exchanges {
        app.requests.selected = Some(exchange.id());
        let _evicted = app.requests.records.push(exchange);
    }

    for size in [
        Layout::WINDOW,
        [800.0, 420.0],
        [700.0, 420.0],
        Layout::MIN_WINDOW,
        [Layout::MIN_WINDOW[0], 900.0],
        [1_100.0, 700.0],
    ] {
        app.detail.tab = DetailTab::Headers;
        let viewport = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(size[0], size[1]));
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
        if (size[0] - Layout::MIN_WINDOW[0]).abs() < 1.0 && (size[1] - 900.0).abs() < 1.0 {
            assert!(
                drawn.iter().any(|item| item.text.contains("x-request-id")),
                "expected wrapped response headers to be visible at 640x900"
            );
        }

        // At 1100x700 after shrinking to 640px, verify the details pane recovered its width.
        if (size[0] - 1_100.0).abs() < 1.0 {
            let headers_tab = drawn
                .iter()
                .find(|item| item.text == "Headers")
                .ok_or("missing Headers tab at 1100x700")?;
            // A 38% detail share on 1100px puts the details pane and its
            // first tab around x ≈ 690, well left of the cramped ~780px
            // position a stuck narrow panel would keep.
            assert!(
                headers_tab.rect.left() < 760.0,
                "details pane did not recover width on enlarge: {:#?}",
                headers_tab.rect
            );
        }
    }

    for mode in body_format::ViewMode::ALL {
        app.detail.tab = DetailTab::Body;
        app.detail.body_mode = mode;
        let viewport = egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(Layout::MIN_WINDOW[0], Layout::MIN_WINDOW[1]),
        );
        let drawn = render_settled(&mut app, Layout::MIN_WINDOW);
        assert_no_text_overlap(&drawn, viewport, &format!("body-{mode:?}@640x420"));
    }

    // Empty list and unselected detail pane at minimum window size.
    app.requests.records.clear(0);
    app.requests.selected = None;
    let min_viewport = egui::Rect::from_min_size(
        egui::Pos2::ZERO,
        egui::vec2(Layout::MIN_WINDOW[0], Layout::MIN_WINDOW[1]),
    );
    let drawn = render_settled(&mut app, Layout::MIN_WINDOW);
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
        app.requests.selected = Some(exchange.id());
        let _evicted = app.requests.records.push(exchange);
    }

    let wide = [1_100.0, 700.0];
    let initial_tab_left = text_left(&render_settled(&mut app, wide), "Headers")?;

    // CentralPanel defaults to 8px horizontal margin on each side.
    let main_width = wide[0] - 16.0;
    let start_x =
        (8.0 + main_width - split_detail_width(main_width, app.layout.detail_share)).round();
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
        app.layout.detail_share > 0.50,
        "expected dragged detail_share > 0.50, got {}",
        app.layout.detail_share
    );
    let dragged_share = app.layout.detail_share;
    let dragged_tab_left = text_left(&after_drag, "Headers")?;
    assert!(
        dragged_tab_left < initial_tab_left - 100.0,
        "details pane should widen immediately after drag release: before={initial_tab_left}, after={dragged_tab_left}"
    );

    // Shrink to minimum size: clamped cleanly without overwriting the user's preferred share.
    let min_viewport = egui::Rect::from_min_size(
        egui::Pos2::ZERO,
        egui::vec2(Layout::MIN_WINDOW[0], Layout::MIN_WINDOW[1]),
    );
    let narrow_drawn = render_settled(&mut app, Layout::MIN_WINDOW);
    assert_no_text_overlap(&narrow_drawn, min_viewport, "dragged-shrunk@640x420");
    assert!((app.layout.detail_share - dragged_share).abs() < f32::EPSILON);

    // Enlarge back to wide: user-dragged split width is restored.
    let wide_viewport = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(wide[0], wide[1]));
    let restored_drawn = render_settled(&mut app, wide);
    assert_no_text_overlap(&restored_drawn, wide_viewport, "dragged-restored@1100x700");
    let restored_tab_left = text_left(&restored_drawn, "Headers")?;
    assert!(
        (restored_tab_left - dragged_tab_left).abs() < 4.0,
        "expected restored detail position near {dragged_tab_left}, got {restored_tab_left}"
    );

    Ok(())
}

#[tokio::test]
async fn dialogs_fit_minimum_window_without_text_overlap() -> Result<(), Box<dyn std::error::Error>>
{
    let exchanges = timeout(Duration::from_secs(5), capture_sample_exchanges()).await??;
    let sample = exchanges.last().cloned().ok_or("missing sample exchange")?;
    let viewport = egui::Rect::from_min_size(
        egui::Pos2::ZERO,
        egui::vec2(Layout::MIN_WINDOW[0], Layout::MIN_WINDOW[1]),
    );

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
            let _ = har.show(
                ui.ctx(),
                counts,
                false,
                true,
                crate::ui::i18n::Language::English,
            );
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
            let _ = replay.show(ui.ctx(), false, None, crate::ui::i18n::Language::English);
        });
        output.textures_delta.clear();
        drawn = collect_text_shapes(&output.shapes);
    }
    assert_no_text_overlap(&drawn, viewport, "replay-panel@640x420");

    // Bulk body save modal at 640x420.
    let ctx = egui::Context::default();
    let mut app = test_app(ctx);
    app.session.body_saving.open_bulk();
    let mut drawn = Vec::new();
    for _ in 0..3 {
        let input = egui::RawInput {
            screen_rect: Some(viewport),
            ..Default::default()
        };
        let mut output = app.context.clone().run_ui(input, |ui| {
            let _ = app.show_bulk_save(ui.ctx());
        });
        output.textures_delta.clear();
        drawn = collect_text_shapes(&output.shapes);
    }
    assert_no_text_overlap(&drawn, viewport, "bulk-save@640x420");
    assert!(drawn.iter().any(|item| item.text == "Save"));
    assert!(drawn.iter().any(|item| item.text == "Cancel"));

    Ok(())
}

async fn capture_post_with_json_bodies() -> Result<Vec<Exchange>, Box<dyn std::error::Error>> {
    let upstream = TcpListener::bind("127.0.0.1:0").await?;
    let address = upstream.local_addr()?;
    let mut proxy = ProxyHandle::start(ProxyConfig::loopback(0))?;
    let mut captured = Vec::new();

    // First exchange so ArrowUp can switch away from the second exchange.
    let cases: [(&str, &str, &str, &str); 2] = [
        (
            "GET",
            "/api/v2/health",
            "",
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 15\r\nConnection: close\r\n\r\n{\"status\":\"ok\"}",
        ),
        (
            "POST",
            "/api/v2/messages",
            "{\"sender\":\"alice\",\"text\":\"hello  world\"}",
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 39\r\nConnection: close\r\n\r\n{\"accepted\":true,\"reply\":\"saved  item\"}",
        ),
    ];

    for (method, path, req_body, response) in cases {
        let mut client = TcpStream::connect(proxy.local_addr()).await?;
        let request = format!(
            "{method} http://{address}{path} HTTP/1.1\r\nHost: {address}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{req_body}",
            req_body.len()
        );
        client.write_all(request.as_bytes()).await?;
        let (mut server, _) = upstream.accept().await?;
        let record = proxy.receive().await.ok_or("missing captured exchange")?;
        read_head(&mut server).await?;
        if !req_body.is_empty() {
            let mut body_buf = vec![0; req_body.len()];
            server.read_exact(&mut body_buf).await?;
        }
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

fn run_app_frame(
    app: &mut InspectorApp,
    size: [f32; 2],
    time: f64,
    events: Vec<egui::Event>,
) -> egui::FullOutput {
    let viewport = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(size[0], size[1]));
    let input = egui::RawInput {
        time: Some(time),
        screen_rect: Some(viewport),
        events,
        ..Default::default()
    };
    let mut output = app.context.clone().run_ui(input, |ui| {
        egui::CentralPanel::default().show(ui, |ui| app.render_ui(ui));
    });
    output.textures_delta.clear();
    output
}

fn find_galley(
    shapes: &[egui::epaint::ClippedShape],
    needle: &str,
) -> Option<(egui::Pos2, std::sync::Arc<egui::Galley>)> {
    fn walk(
        shape: &egui::Shape,
        needle: &str,
    ) -> Option<(egui::Pos2, std::sync::Arc<egui::Galley>)> {
        match shape {
            egui::Shape::Text(text) if text.galley.text().contains(needle) => {
                Some((text.pos, std::sync::Arc::clone(&text.galley)))
            }
            egui::Shape::Vec(items) => items.iter().find_map(|item| walk(item, needle)),
            _ => None,
        }
    }
    shapes
        .iter()
        .find_map(|clipped| walk(&clipped.shape, needle))
}

fn galley_cursor_pos(
    origin: egui::Pos2,
    galley: &egui::Galley,
    byte: usize,
) -> Result<egui::Pos2, String> {
    let ch = galley
        .text()
        .get(..byte)
        .ok_or_else(|| format!("invalid offset {byte}"))?
        .chars()
        .count();
    let rect = galley.pos_from_cursor(egui::text::CCursor::new(ch));
    Ok(egui::pos2(
        origin.x + rect.min.x,
        origin.y + rect.center().y,
    ))
}

fn drag_copy_in_app(
    app: &mut InspectorApp,
    size: [f32; 2],
    time: &mut f64,
    shapes: &[egui::epaint::ClippedShape],
    slice: &str,
) -> Result<Vec<String>, String> {
    let (origin, galley) =
        find_galley(shapes, slice).ok_or_else(|| format!("missing galley containing {slice:?}"))?;
    let start = galley
        .text()
        .find(slice)
        .ok_or_else(|| format!("missing slice {slice:?}"))?;
    let from = galley_cursor_pos(origin, &galley, start)?;
    let to = galley_cursor_pos(origin, &galley, start + slice.len())?;
    for events in [
        vec![egui::Event::PointerMoved(from)],
        vec![egui::Event::PointerButton {
            pos: from,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::NONE,
        }],
        vec![egui::Event::PointerMoved(to)],
        vec![egui::Event::PointerButton {
            pos: to,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        }],
    ] {
        *time += 0.05;
        let _ = run_app_frame(app, size, *time, events);
    }
    *time += 0.05;
    let out = run_app_frame(app, size, *time, vec![egui::Event::Copy]);
    Ok(out
        .platform_output
        .commands
        .iter()
        .filter_map(|cmd| match cmd {
            egui::OutputCommand::CopyText(text) => Some(text.clone()),
            _ => None,
        })
        .collect())
}

fn press_key(app: &mut InspectorApp, size: [f32; 2], time: &mut f64, key: egui::Key) {
    *time += 0.05;
    let _ = run_app_frame(
        app,
        size,
        *time,
        vec![egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }],
    );
}

#[test]
fn escape_closes_the_body_modal_before_export_har() {
    let mut app = test_app(egui::Context::default());
    let size = Layout::WINDOW;
    let mut time = 0.05;
    app.dialogs.har.open("edited-fixture.har".to_owned());
    let _ = run_app_frame(&mut app, size, time, Vec::new());

    app.session.body_saving.open_bulk();
    time += 0.05;
    let _ = run_app_frame(&mut app, size, time, Vec::new());
    press_key(&mut app, size, &mut time, egui::Key::Escape);
    assert!(!app.session.body_saving.dialog_is_open());
    assert!(
        app.dialogs.har.is_open(),
        "Escape closed the HAR draft under the modal"
    );

    time += 0.05;
    let output = run_app_frame(&mut app, size, time, Vec::new());
    assert!(find_galley(&output.shapes, "edited-fixture.har").is_some());

    press_key(&mut app, size, &mut time, egui::Key::Escape);
    assert!(!app.dialogs.har.is_open());
}

#[test]
fn dialogs_suspend_request_shortcuts_until_they_close() -> Result<(), Box<dyn std::error::Error>> {
    use http::Method;
    use http_inspector_exchange::ExchangeBuilder;

    let mut app = test_app(egui::Context::default());
    let first = ExchangeBuilder::http(Method::GET, "http://fixture.test/first")?.in_flight()?;
    let second = ExchangeBuilder::http(Method::GET, "http://fixture.test/second")?.in_flight()?;
    let first_id = first.id();
    let second_id = second.id();
    let _ = app.requests.record_added(first);
    let _ = app.requests.record_added(second.clone());
    app.requests.selected = Some(second_id);
    let size = Layout::WINDOW;
    let mut time = 0.0;

    app.session.body_saving.open_bulk();
    press_key(&mut app, size, &mut time, egui::Key::ArrowUp);
    press_key(&mut app, size, &mut time, egui::Key::F6);
    assert_eq!(app.requests.selected, Some(second_id));
    assert_eq!(app.detail.tab, DetailTab::Headers);
    press_key(&mut app, size, &mut time, egui::Key::Escape);
    assert!(!app.session.body_saving.dialog_is_open());

    app.session.body_saving.open_bulk();
    time += 0.05;
    let _ = run_app_frame(&mut app, size, time, Vec::new());
    let backdrop = egui::pos2(30.0, 500.0);
    for pressed in [true, false] {
        time += 0.05;
        let _ = run_app_frame(
            &mut app,
            size,
            time,
            vec![
                egui::Event::PointerMoved(backdrop),
                egui::Event::PointerButton {
                    pos: backdrop,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
    }
    assert!(!app.session.body_saving.dialog_is_open());

    app.dialogs.har.open("fixture.har".to_owned());
    press_key(&mut app, size, &mut time, egui::Key::ArrowUp);
    press_key(&mut app, size, &mut time, egui::Key::F6);
    assert_eq!(app.requests.selected, Some(second_id));
    assert_eq!(app.detail.tab, DetailTab::Headers);
    press_key(&mut app, size, &mut time, egui::Key::Escape);
    assert!(!app.dialogs.har.is_open());

    app.detail.open_replay(second)?;
    press_key(&mut app, size, &mut time, egui::Key::ArrowUp);
    press_key(&mut app, size, &mut time, egui::Key::F6);
    assert_eq!(app.requests.selected, Some(second_id));
    assert_eq!(app.detail.tab, DetailTab::Headers);

    app.detail = DetailState::default();
    press_key(&mut app, size, &mut time, egui::Key::ArrowUp);
    press_key(&mut app, size, &mut time, egui::Key::F6);
    assert_eq!(app.requests.selected, Some(first_id));
    assert_eq!(app.detail.tab, DetailTab::Body);
    Ok(())
}

#[tokio::test]
async fn body_tab_copy_and_focus_shortcuts_do_not_conflict()
-> Result<(), Box<dyn std::error::Error>> {
    let exchanges = timeout(Duration::from_secs(5), capture_post_with_json_bodies()).await??;
    let first_id = exchanges.first().ok_or("missing first exchange")?.id();
    let post_id = exchanges.get(1).ok_or("missing post exchange")?.id();

    let mut app = test_app(egui::Context::default());
    for exchange in exchanges {
        let _evicted = app.requests.records.push(exchange);
    }
    app.requests.selected = Some(post_id);
    app.detail.tab = DetailTab::Body;
    app.detail.body_mode = body_format::ViewMode::Pretty;

    let size = Layout::WINDOW;
    let mut time = 0.0;
    let mut initial = None;
    for _ in 0..3 {
        time += 0.05;
        initial = Some(run_app_frame(&mut app, size, time, Vec::new()));
    }
    let initial = initial.ok_or("missing initial frame")?;

    let req_target = "\"sender\": \"alice\",\n  \"text\": \"hello  world\"";
    assert_eq!(
        drag_copy_in_app(&mut app, size, &mut time, &initial.shapes, req_target)?,
        vec![req_target.to_owned()]
    );

    let resp_target = "\"accepted\": true,\n  \"reply\": \"saved  item\"";
    assert_eq!(
        drag_copy_in_app(&mut app, size, &mut time, &initial.shapes, resp_target)?,
        vec![resp_target.to_owned()]
    );

    // Focused body text blocks request-list navigation shortcuts so arrows stay in the body.
    assert!(!super::request_shortcuts_allowed(&app.context));
    press_key(&mut app, size, &mut time, egui::Key::ArrowUp);
    assert_eq!(
        app.requests.selected,
        Some(post_id),
        "ArrowUp while body text is focused must not switch selected request"
    );

    // Narrow window with focused body selection must remain overlap-free.
    let min_viewport = egui::Rect::from_min_size(
        egui::Pos2::ZERO,
        egui::vec2(Layout::MIN_WINDOW[0], Layout::MIN_WINDOW[1]),
    );
    time += 0.05;
    let narrow = run_app_frame(&mut app, Layout::MIN_WINDOW, time, Vec::new());
    assert_no_text_overlap(
        &collect_text_shapes(&narrow.shapes),
        min_viewport,
        "body-selected@640x420",
    );

    // Escape surrenders body focus and restores request-list keyboard navigation.
    press_key(&mut app, size, &mut time, egui::Key::Escape);
    assert!(super::request_shortcuts_allowed(&app.context));
    press_key(&mut app, size, &mut time, egui::Key::ArrowUp);
    assert_eq!(
        app.requests.selected,
        Some(first_id),
        "ArrowUp after Escape must resume navigating the request list"
    );
    Ok(())
}

fn seed_synthetic_map_data(map: &mut UrlMap) -> Result<(), Box<dyn std::error::Error>> {
    let synthetic_urls = [
        (
            "https://app.example.test/dashboard?session=SECRET_UI_TOKEN_1",
            None,
            None,
            None,
        ),
        (
            "https://api.example.test/v1/users/101/orders?auth=SECRET_UI_TOKEN_2&page=1",
            Some("https://app.example.test/dashboard?sid=SECRET_UI_REF"),
            Some("https://app.example.test"),
            None,
        ),
        (
            "https://api.example.test/v1/users/202/orders?auth=SECRET_UI_TOKEN_3&page=2",
            Some("https://app.example.test/dashboard?sid=SECRET_UI_REF"),
            Some("https://app.example.test"),
            Some("https://auth.example.test/login?next=SECRET_UI_LOC"),
        ),
        (
            "https://cdn.tracker.net/sdk.js?api_key=SECRET_UI_TOKEN_4",
            Some("https://app.example.test/dashboard"),
            None,
            None,
        ),
    ];
    let base_time = std::time::SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    for (url, referer, origin_hdr, redirect) in synthetic_urls {
        let target = http_inspector_analysis::url::parse_url(url).ok_or("valid synthetic url")?;
        let _ = map.observe(http_inspector_analysis::url_map::ObservedInput {
            target,
            method: http::Method::GET,
            status: http_inspector_analysis::url_map::StatusBucket::S2xx,
            content_type: Some("application/json".to_owned()),
            bytes: 512,
            duration: Some(Duration::from_millis(18)),
            started_at: base_time,
            referer: referer.and_then(http_inspector_analysis::url::parse_url),
            origin_header: origin_hdr.and_then(http_inspector_analysis::url::parse_origin_header),
            redirect_location: redirect.and_then(http_inspector_analysis::url::parse_url),
        });
    }
    Ok(())
}

/// Lists `exchanges` in `app` and maps them together with synthetic
/// cross-site traffic.
fn load_synthetic_map(
    app: &mut InspectorApp,
    exchanges: Vec<Exchange>,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut url_map = UrlMap::new();
    for exchange in exchanges {
        let _ = url_map.observe_exchange(&exchange);
        let _evicted = app.requests.records.push(exchange);
    }
    let _ = url_map.sync_live();
    seed_synthetic_map_data(&mut url_map)?;
    app.map.analysis = AnalysisPipeline::seeded(url_map);
    Ok(())
}

fn assert_no_secrets_painted(drawn: &[DrawnText], label: &str) {
    let secrets = [
        "abcdef0123456789",
        "SECRET_UI_TOKEN_1",
        "SECRET_UI_TOKEN_2",
        "SECRET_UI_TOKEN_3",
        "SECRET_UI_TOKEN_4",
        "SECRET_UI_REF",
        "SECRET_UI_LOC",
    ];
    for item in drawn {
        for secret in secrets {
            assert!(
                !item.text.contains(secret),
                "{label} leaked query value {secret:?} in {:?}",
                item.text
            );
        }
    }
}

#[test]
fn endpoint_filter_marks_unanalyzed_evictions_as_estimates()
-> Result<(), Box<dyn std::error::Error>> {
    use http::Method;
    use http_inspector_analysis::filter::RecordList;
    use http_inspector_analysis::url::{OriginKey, PathSegmentPattern};
    use http_inspector_analysis::url_map::{RequestScope, ViewNodeKey};
    use http_inspector_exchange::ExchangeBuilder;

    let mut app = test_app(egui::Context::default());
    app.requests.records = RecordList::new(1);
    let key = ViewNodeKey {
        origin: OriginKey::new("http", "fixture.test", None),
        path: vec![PathSegmentPattern::Literal("api".to_owned())],
    };
    for _ in 0..2 {
        let record = ExchangeBuilder::http(Method::GET, "http://fixture.test/api")?.in_flight()?;
        app.map.analysis.record_added(&record);
        if let Some(evicted) = app.requests.record_added(record) {
            app.map.analysis.record_evicted(&evicted);
        }
    }

    app.requests
        .show_map_scope(RequestScope::endpoint(key.clone(), Method::GET));
    let drawn = render_settled(&mut app, Layout::WINDOW);
    assert!(
        drawn
            .iter()
            .any(|item| item.text == "(1 retained · ≈0 older removed)")
    );

    app.requests.show_map_scope(RequestScope::subtree(key));
    let drawn = render_settled(&mut app, Layout::WINDOW);
    assert!(
        drawn
            .iter()
            .any(|item| item.text == "(1 retained · 1 older removed)")
    );
    Ok(())
}

#[tokio::test]
async fn map_detail_shows_endpoint_structure_without_values()
-> Result<(), Box<dyn std::error::Error>> {
    let exchanges = timeout(Duration::from_secs(5), capture_sample_exchanges()).await??;
    let context = egui::Context::default();
    let mut app = test_app(context.clone());
    let wake_context = context.clone();
    app.map.analysis = AnalysisPipeline::start(Arc::new(PreviewBodies), move || {
        wake_context.request_repaint();
    });
    for exchange in exchanges {
        app.map.analysis.record_added(&exchange);
        let _evicted = app.requests.records.push(exchange);
    }
    timeout(Duration::from_secs(5), async {
        while app.map.analysis.endpoint_schemas().version() < 3 {
            let _changed = app.map.analysis.update();
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await?;

    let origin = app
        .map
        .analysis
        .url_map()
        .origins()
        .keys()
        .next()
        .cloned()
        .ok_or("missing origin")?;
    app.layout.main_view = MainView::Map;
    app.map.panel.sub_view = super::MapSubView::Tree;
    app.map.panel.selected = Some(http_inspector_analysis::url_map::ViewNodeKey {
        origin,
        path: ["api", "v2", "users"]
            .into_iter()
            .map(|segment| {
                http_inspector_analysis::url::PathSegmentPattern::Literal(segment.to_owned())
            })
            .collect(),
    });
    let tall = [Layout::WINDOW[0], 2_000.0];
    for size in [Layout::WINDOW, Layout::MIN_WINDOW, tall] {
        let viewport = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(size[0], size[1]));
        let drawn = render_settled(&mut app, size);
        assert_no_text_overlap(&drawn, viewport, &format!("map-endpoints@{size:?}"));
        assert_no_secrets_painted(&drawn, &format!("map-endpoints@{size:?}"));
        for value in ["Alice", "Bob", "active", "created_at_desc"] {
            assert!(
                drawn.iter().all(|item| !item.text.contains(value)),
                "painted value {value:?}"
            );
        }
    }
    let drawn = render_settled(&mut app, tall);
    let texts: Vec<&str> = drawn.iter().map(|item| item.text.as_str()).collect();
    for expected in ["Endpoints", "GET · 1 request(s)", "OPTIONS · 1 request(s)"] {
        assert!(
            texts.contains(&expected),
            "missing {expected:?} in {texts:?}"
        );
    }
    assert!(
        texts.iter().any(|text| text.starts_with("users: array")),
        "missing users field in {texts:?}"
    );
    Ok(())
}

#[tokio::test]
async fn map_views_avoid_overlap_never_leak_query_values_and_filter_requests()
-> Result<(), Box<dyn std::error::Error>> {
    let exchanges = timeout(Duration::from_secs(5), capture_sample_exchanges()).await??;
    let mut app = test_app(egui::Context::default());
    load_synthetic_map(&mut app, exchanges)?;

    app.layout.main_view = MainView::Map;
    let first_origin = app
        .map
        .analysis
        .url_map()
        .origins()
        .keys()
        .next()
        .cloned()
        .ok_or("missing origin")?;
    app.map.panel.selected = Some(http_inspector_analysis::url_map::ViewNodeKey {
        origin: first_origin,
        path: Vec::new(),
    });

    for size in [Layout::WINDOW, Layout::MIN_WINDOW] {
        let viewport = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(size[0], size[1]));
        for (sub, tag) in [
            (super::MapSubView::Tree, "map-tree"),
            (super::MapSubView::Graph, "map-graph"),
        ] {
            app.map.panel.sub_view = sub;
            let drawn = render_settled(&mut app, size);
            assert!(!drawn.is_empty());
            assert_no_text_overlap(&drawn, viewport, &format!("{tag}@{size:?}"));
            assert_no_secrets_painted(&drawn, &format!("{tag}@{size:?}"));
        }
    }

    app.map.panel.sub_view = super::MapSubView::Tree;
    let mut time = 0.0;
    let _ = run_app_frame(&mut app, Layout::WINDOW, time, Vec::new());
    press_key(&mut app, Layout::WINDOW, &mut time, egui::Key::ArrowDown);
    assert!(app.map.panel.selected.is_some());
    press_key(&mut app, Layout::WINDOW, &mut time, egui::Key::ArrowRight);
    let _ = run_app_frame(&mut app, Layout::WINDOW, time + 0.05, Vec::new());

    let local_origin = app
        .map
        .analysis
        .url_map()
        .origins()
        .keys()
        .find(|origin| origin.host() == "127.0.0.1")
        .cloned()
        .ok_or("missing local origin")?;
    let users_key = http_inspector_analysis::url_map::ViewNodeKey {
        origin: local_origin,
        path: vec![
            http_inspector_analysis::url::PathSegmentPattern::Literal("api".to_owned()),
            http_inspector_analysis::url::PathSegmentPattern::Literal("v2".to_owned()),
            http_inspector_analysis::url::PathSegmentPattern::Literal("users".to_owned()),
        ],
    };
    app.handle_map_action(
        &app.context.clone(),
        super::MapAction::ShowInRequests(http_inspector_analysis::url_map::RequestScope::subtree(
            users_key,
        )),
    );
    assert_eq!(app.layout.main_view, MainView::Requests);
    assert_eq!(app.requests.filtered_records().len(), 2);

    let min_viewport = egui::Rect::from_min_size(
        egui::Pos2::ZERO,
        egui::vec2(Layout::MIN_WINDOW[0], Layout::MIN_WINDOW[1]),
    );
    let filtered_drawn = render_settled(&mut app, Layout::MIN_WINDOW);
    assert_no_text_overlap(&filtered_drawn, min_viewport, "requests-map-filter@640x420");
    assert!(
        filtered_drawn
            .iter()
            .any(|item| item.text.contains("Map filter:"))
    );

    app.clear_session();
    assert_eq!(app.requests.records.len(), 0);
    assert_eq!(app.map.analysis.url_map().node_count(), 0);
    assert!(app.requests.map_path_filter.is_none());
    Ok(())
}

fn seed_scale_workload(map: &mut UrlMap, count: usize) -> Duration {
    use http_inspector_analysis::url::{parse_origin_header, parse_url};
    use http_inspector_analysis::url_map::{ObservedInput, StatusBucket};
    use std::time::{Instant, SystemTime};

    let hosts = [
        "app.example.com",
        "api.example.com",
        "auth.example.com",
        "static.example.com",
        "cdn.tracker.net",
        "metrics.analytics.io",
        "payments.partner.org",
        "media.storage.net",
    ];
    let resources = ["users", "orders", "items", "events", "sessions", "comments"];
    let base_time = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let start = Instant::now();
    for i in 0..count {
        let host = hosts
            .get(i % hosts.len())
            .copied()
            .unwrap_or("app.example.com");
        let resource = resources
            .get((i / 7) % resources.len())
            .copied()
            .unwrap_or("items");
        let sub_id = i;
        let url_str = match i % 4 {
            0 => format!("https://{host}/api/v1/{resource}/{sub_id}?page={}", i % 10),
            1 => format!("https://{host}/api/v1/{resource}/{sub_id}/details"),
            2 => format!("https://{host}/assets/chunk-{sub_id:04}.js"),
            _ => format!("https://{host}/pages/section-{}/view-{}", i % 80, i % 120),
        };
        let Some(target) = parse_url(&url_str) else {
            continue;
        };
        let _ = map.observe(ObservedInput {
            target,
            method: http::Method::GET,
            status: StatusBucket::S2xx,
            content_type: Some("application/json".to_owned()),
            bytes: 512,
            duration: Some(Duration::from_millis(20)),
            started_at: base_time + Duration::from_millis(u64::try_from(i).unwrap_or(0)),
            referer: (i % 3 == 0)
                .then(|| parse_url("https://app.example.com/dashboard"))
                .flatten(),
            origin_header: (i % 5 == 0)
                .then(|| parse_origin_header("https://app.example.com"))
                .flatten(),
            redirect_location: (i % 17 == 0)
                .then(|| parse_url("https://auth.example.com/login"))
                .flatten(),
        });
    }
    start.elapsed()
}

fn measure_frames(app: &mut InspectorApp, time: &mut f64, frames: usize) -> Vec<Duration> {
    let mut samples = Vec::with_capacity(frames);
    for _ in 0..frames {
        *time += 0.016;
        let start = std::time::Instant::now();
        let _ = run_app_frame(app, Layout::WINDOW, *time, Vec::new());
        samples.push(start.elapsed());
    }
    samples.sort();
    samples
}

async fn capture_bulk_exchanges(
    count: usize,
) -> Result<(Vec<Exchange>, http_inspector_analysis::url::OriginKey), Box<dyn std::error::Error>> {
    let upstream = TcpListener::bind("127.0.0.1:0").await?;
    let address = upstream.local_addr()?;
    let server_task = tokio::spawn(async move {
        let Ok((mut socket, _)) = upstream.accept().await else {
            return;
        };
        let body = b"{\"ok\":true}";
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: keep-alive\r\n\r\n",
            body.len()
        );
        let mut head = Vec::with_capacity(512);
        let mut byte = [0; 1];
        for _ in 0..count {
            head.clear();
            while !head.ends_with(b"\r\n\r\n") {
                if socket.read_exact(&mut byte).await.is_err() {
                    return;
                }
                head.extend_from_slice(&byte);
            }
            if socket.write_all(response.as_bytes()).await.is_err()
                || socket.write_all(body).await.is_err()
            {
                return;
            }
        }
    });

    let mut proxy = ProxyHandle::start(ProxyConfig::loopback(0))?;
    let mut client = TcpStream::connect(proxy.local_addr()).await?;
    let mut captured = Vec::with_capacity(count);
    let mut resp_buf = Vec::with_capacity(256);
    let mut byte = [0; 1];
    for i in 0..count {
        let req = format!(
            "GET http://{address}/api/v1/users/{i}/orders?page={} HTTP/1.1\r\nHost: {address}\r\nConnection: keep-alive\r\n\r\n",
            i % 10
        );
        client.write_all(req.as_bytes()).await?;
        resp_buf.clear();
        while !resp_buf.ends_with(b"\r\n\r\n") {
            client.read_exact(&mut byte).await?;
            resp_buf.extend_from_slice(&byte);
        }
        let mut body_sink = [0; 11];
        client.read_exact(&mut body_sink).await?;
        if let Some(record) = proxy.receive().await {
            captured.push(record);
        }
    }
    drop(client);
    let _ = server_task.await;
    proxy.stop()?;
    let origin =
        http_inspector_analysis::url::OriginKey::new("http", "127.0.0.1", Some(address.port()));
    Ok((captured, origin))
}

#[tokio::test]
#[ignore = "manual performance measurement; run in release mode with --ignored --nocapture"]
#[allow(clippy::print_stderr, reason = "manual performance measurement output")]
async fn measure_map_ui_frame_time_at_scale() -> Result<(), Box<dyn std::error::Error>> {
    let mut app = test_app(egui::Context::default());
    let (exchanges, local_origin) = capture_bulk_exchanges(MAX_RECORDS).await?;
    let mut url_map = UrlMap::new();
    for record in exchanges {
        let _ = url_map.observe_exchange(&record);
        let _evicted = app.requests.records.push(record);
    }
    let _ = url_map.sync_live();
    let insert_elapsed = seed_scale_workload(&mut url_map, 3_000);
    app.map.analysis = AnalysisPipeline::seeded(url_map);
    let node_count = app.map.analysis.url_map().node_count();
    let retained_count = app.requests.records.len();

    let selected_key = http_inspector_analysis::url_map::ViewNodeKey {
        origin: local_origin,
        path: Vec::new(),
    };
    app.layout.main_view = MainView::Map;
    app.map.panel.sub_view = super::MapSubView::Tree;
    app.map.panel.selected = Some(selected_key.clone());

    let mut time = 0.0;
    let tree_cold = measure_frames(&mut app, &mut time, 1);
    let tree_frames = measure_frames(&mut app, &mut time, 60);

    app.map.panel.sub_view = super::MapSubView::Graph;
    let graph_frames = measure_frames(&mut app, &mut time, 60);

    app.handle_map_action(
        &app.context.clone(),
        super::MapAction::ShowInRequests(http_inspector_analysis::url_map::RequestScope::subtree(
            selected_key,
        )),
    );
    let req_cold = measure_frames(&mut app, &mut time, 1);
    let req_frames = measure_frames(&mut app, &mut time, 60);

    let ms = |d: Option<&Duration>| d.map_or(0.0, |dur| dur.as_secs_f64() * 1_000.0);
    eprintln!(
        "URL map realistic scale measurement ({retained_count} retained records + 3,000 synthetic = 5,000 requests, {node_count} nodes):\n  \
         insert_3000_synth={:.2}ms ({:.2}us/req)\n  \
         tree_selected_cold={:.3}ms | steady p50={:.3}ms p95={:.3}ms max={:.3}ms\n  \
         graph_steady p50={:.3}ms p95={:.3}ms max={:.3}ms\n  \
         requests_map_filter_cold={:.3}ms | steady p50={:.3}ms p95={:.3}ms max={:.3}ms",
        insert_elapsed.as_secs_f64() * 1_000.0,
        insert_elapsed.as_secs_f64() * 1_000_000.0 / 3_000.0,
        ms(tree_cold.first()),
        ms(tree_frames.get(30)),
        ms(tree_frames.get(57)),
        ms(tree_frames.last()),
        ms(graph_frames.get(30)),
        ms(graph_frames.get(57)),
        ms(graph_frames.last()),
        ms(req_cold.first()),
        ms(req_frames.get(30)),
        ms(req_frames.get(57)),
        ms(req_frames.last()),
    );
    Ok(())
}

#[tokio::test]
async fn korean_layout_has_no_text_overlap_in_default_and_narrow_windows()
-> Result<(), Box<dyn std::error::Error>> {
    use crate::ui::i18n::{Language, LanguageSettings};

    let mut app = test_app(egui::Context::default());
    app.layout.language = LanguageSettings::fixed(Language::Korean);
    for record in capture_sample_exchanges().await? {
        app.map.analysis.record_added(&record);
        let _evicted = super::capture::add_record(
            &mut app.requests.records,
            &mut app.requests.selected,
            record,
        );
    }

    for (label, size) in [
        ("korean-default", Layout::WINDOW),
        ("korean-narrow", Layout::MIN_WINDOW),
    ] {
        let viewport = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(size[0], size[1]));
        app.layout.main_view = MainView::Requests;
        for tab in [DetailTab::Headers, DetailTab::Body] {
            app.detail.tab = tab;
            let drawn = render_settled(&mut app, size);
            assert_no_text_overlap(&drawn, viewport, &format!("{label}-requests-{tab:?}"));
            assert!(
                drawn.iter().any(|item| item.text.contains("메서드")),
                "{label}: expected Korean column header in Requests view"
            );
        }

        app.layout.main_view = MainView::Map;
        app.map.panel.sub_view = super::MapSubView::Tree;
        let drawn = render_settled(&mut app, size);
        assert_no_text_overlap(&drawn, viewport, &format!("{label}-map-tree"));
        assert!(
            drawn.iter().any(|item| item.text.starts_with("맵")),
            "{label}: expected Korean Map tab label"
        );
        assert!(
            drawn.iter().any(|item| item.text.contains("트리")),
            "{label}: expected Korean Tree view label"
        );

        app.map.panel.sub_view = super::MapSubView::Graph;
        let drawn = render_settled(&mut app, size);
        assert_no_text_overlap(&drawn, viewport, &format!("{label}-map-graph"));
    }
    Ok(())
}

#[test]
fn switching_language_updates_ui_immediately_without_restart() {
    use crate::ui::i18n::{Language, LanguagePreference, LanguageSettings};

    let mut app = test_app(egui::Context::default());
    app.layout.language = LanguageSettings::fixed(Language::English);

    let english_drawn = render_settled(&mut app, Layout::WINDOW);
    assert!(english_drawn.iter().any(|item| item.text == "Requests"));
    assert!(english_drawn.iter().any(|item| item.text == "Settings"));

    let ctx = app.context.clone();
    app.layout
        .language
        .set_preference(&ctx, LanguagePreference::Korean);
    let (korean_drawn, _) = render_frame(&mut app, Layout::WINDOW, Vec::new());
    assert!(korean_drawn.iter().any(|item| item.text == "요청"));
    assert!(korean_drawn.iter().any(|item| item.text == "설정"));
    assert!(!korean_drawn.iter().any(|item| item.text == "Requests"));

    app.layout
        .language
        .set_preference(&ctx, LanguagePreference::English);
    let (restored_drawn, _) = render_frame(&mut app, Layout::WINDOW, Vec::new());
    assert!(restored_drawn.iter().any(|item| item.text == "Requests"));
    assert!(!restored_drawn.iter().any(|item| item.text == "요청"));
}

/// Vertical center of the first drawn text equal to `label`.
fn text_center_y(drawn: &[DrawnText], label: &str) -> Result<f32, String> {
    drawn
        .iter()
        .find(|item| item.text == label)
        .map(|item| item.rect.center().y)
        .ok_or_else(|| format!("missing {label:?} text"))
}

#[tokio::test]
async fn shell_bands_keep_their_heights_and_positions_in_both_themes()
-> Result<(), Box<dyn std::error::Error>> {
    let exchanges = timeout(Duration::from_secs(5), capture_sample_exchanges()).await??;
    let mut app = test_app(egui::Context::default());
    for exchange in exchanges {
        app.requests.selected = Some(exchange.id());
        let _evicted = app.requests.records.push(exchange);
    }
    let count = format!("{} requests", app.requests.records.len());
    // `render_frame` wraps the app in a CentralPanel with an 8px margin.
    let margin = 8.0;
    let size = Layout::WINDOW;
    let toolbar = margin..=margin + Layout::TOOLBAR_HEIGHT;
    let header = *toolbar.end()..=*toolbar.end() + Layout::PANE_HEADER_HEIGHT;
    let status = size[1] - margin - Layout::STATUS_BAR_HEIGHT..=size[1] - margin;

    let mut positions = Vec::new();
    for theme in [egui::Theme::Dark, egui::Theme::Light] {
        app.context.set_theme(theme);
        let drawn = render_settled(&mut app, size);
        let placed = [
            (http_inspector_exchange::APP_NAME, &toolbar),
            ("Requests", &toolbar),
            ("Settings", &toolbar),
            ("All", &header),
            ("Headers", &header),
            (count.as_str(), &status),
        ]
        .map(|(label, band)| {
            text_center_y(&drawn, label).inspect(|y| {
                assert!(
                    band.contains(y),
                    "{theme:?}: {label:?} at y={y}, outside {band:?}"
                );
            })
        });
        positions.push(placed.into_iter().collect::<Result<Vec<_>, _>>()?);
    }
    // Light and dark differ in color only (D22).
    assert_eq!(positions.first(), positions.last());
    Ok(())
}

#[test]
fn narrow_toolbar_moves_the_view_switch_and_filter_to_a_second_row() {
    let mut app = test_app(egui::Context::default());
    let drawn = render_settled(&mut app, Layout::MIN_WINDOW);
    let margin = 8.0;
    let first_row = margin + Layout::TOOLBAR_HEIGHT;
    for label in [
        "Requests",
        crate::ui::workbench::text::ShellText::Filter.text(crate::ui::i18n::Language::English),
    ] {
        let y = text_center_y(&drawn, label).unwrap_or_default();
        assert!(
            y > first_row && y < first_row + Layout::TOOLBAR_HEIGHT,
            "{label} at y={y}"
        );
    }
    let viewport = egui::Rect::from_min_size(
        egui::Pos2::ZERO,
        egui::vec2(Layout::MIN_WINDOW[0], Layout::MIN_WINDOW[1]),
    );
    assert_no_text_overlap(&drawn, viewport, "toolbar@640x420");
}

#[test]
fn invalid_pattern_is_reported_inside_the_filter_input() {
    use http_inspector_analysis::search::{MatchOptions, SearchScope};
    let mut app = test_app(egui::Context::default());
    let options = MatchOptions {
        regex: true,
        ..MatchOptions::default()
    };
    let context = app.context.clone();
    app.requests.search.search_now(
        ("users[", SearchScope::All, options),
        &app.requests.records,
        &context,
    );
    let drawn = render_settled(&mut app, Layout::WINDOW);
    let error = drawn
        .iter()
        .find(|item| item.text == "Invalid pattern")
        .map(|item| item.rect);
    let toolbar_bottom = 8.0 + Layout::TOOLBAR_HEIGHT;
    assert!(
        error.is_some_and(|rect| rect.bottom() <= toolbar_bottom),
        "expected the short error inside the toolbar input, got {error:?}"
    );
    // The parser message is the input's tooltip, not a line of its own.
    assert!(!drawn.iter().any(|item| item.text.contains("error parsing")));
}

#[test]
fn shell_tabs_are_announced_as_tab_lists() -> Result<(), Box<dyn std::error::Error>> {
    let mut app = test_app(egui::Context::default());
    app.context.enable_accesskit();
    let viewport = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1_280.0, 820.0));
    let mut tree = None;
    for _ in 0..3 {
        let input = egui::RawInput {
            screen_rect: Some(viewport),
            ..Default::default()
        };
        let mut output = app.context.clone().run_ui(input, |ui| {
            egui::CentralPanel::default().show(ui, |ui| app.render_ui(ui));
        });
        output.textures_delta.clear();
        tree = output.platform_output.accesskit_update;
    }
    let tree = tree.ok_or("missing accessibility tree")?;
    let node = |id| {
        tree.nodes
            .iter()
            .find(|(node_id, _)| *node_id == id)
            .map(|(_, node)| node)
    };
    let tabs_of = |name: &str| -> Option<Vec<(String, Option<bool>)>> {
        let (_, list) = tree
            .nodes
            .iter()
            .find(|(_, node)| node.role() == egui::Role::TabList && node.label() == Some(name))?;
        Some(
            list.children()
                .iter()
                .filter_map(|&id| node(id))
                .filter(|tab| tab.role() == egui::Role::Tab)
                .map(|tab| {
                    (
                        tab.label().unwrap_or_default().to_owned(),
                        tab.is_selected(),
                    )
                })
                .collect(),
        )
    };
    let selected = |label: &str, on: bool| (label.to_owned(), Some(on));
    assert_eq!(
        tabs_of("View").ok_or("missing View tab list")?,
        vec![selected("Requests", true), selected("Map", false)]
    );
    assert_eq!(
        tabs_of("Request outcome").ok_or("missing Request outcome tab list")?,
        vec![
            selected("All", true),
            selected("Responses", false),
            selected("Failures", false),
            selected("Tunnels", false),
        ]
    );
    assert_eq!(
        tabs_of("Request details").ok_or("missing Request details tab list")?,
        vec![
            selected("Headers", true),
            selected("Body", false),
            selected("Timing", false),
        ]
    );
    Ok(())
}

#[test]
fn structured_filter_displays_the_actual_combined_count_and_diagnostic()
-> Result<(), Box<dyn std::error::Error>> {
    use http::{Method, StatusCode};
    use http_inspector_analysis::filter::RecordFilter;
    use http_inspector_analysis::search::{MatchOptions, SearchScope};
    use http_inspector_exchange::{ExchangeBuilder, ResponseFixture};

    let mut app = test_app(egui::Context::default());
    for (method, path, status) in [
        (Method::POST, "/api/users", StatusCode::NOT_FOUND),
        (Method::POST, "/api/orders", StatusCode::NOT_FOUND),
        (Method::GET, "/api/users", StatusCode::NOT_FOUND),
    ] {
        let record = ExchangeBuilder::http(method, format!("http://localhost:8080{path}"))?
            .respond(ResponseFixture::new(status))?;
        let _evicted = app.requests.records.push(record);
    }
    let context = app.context.clone();
    app.requests.search.search_now(
        (
            "method:POST url:/api/users status:abc localhost:8080",
            SearchScope::All,
            MatchOptions::default(),
        ),
        &app.requests.records,
        &context,
    );
    app.requests.record_filter = RecordFilter::Responses;
    let filtered = app.requests.filtered_records();
    assert_eq!(filtered.len(), 1);
    assert_eq!(
        filtered.first().map(|record| record.request().method()),
        Some(&Method::POST)
    );

    for size in [Layout::WINDOW, Layout::MIN_WINDOW] {
        let drawn = render_settled(&mut app, size);
        assert!(drawn.iter().any(|item| item.text == "1 of 3 requests"));
        assert!(drawn.iter().any(|item| item.text == "Invalid condition"));
        assert!(!drawn.iter().any(|item| item.text == "3 requests"));
    }
    app.requests.record_filter = RecordFilter::Failures;
    assert!(app.requests.filtered_records().is_empty());
    Ok(())
}

#[test]
#[ignore = "manual performance measurement; run in release mode with --ignored --nocapture"]
#[allow(clippy::print_stderr, reason = "manual performance measurement output")]
fn measure_structured_filter_at_scale() -> Result<(), Box<dyn std::error::Error>> {
    use http::header::{CONTENT_TYPE, HeaderValue};
    use http::{Method, StatusCode};
    use http_inspector_analysis::search::{MatchOptions, SearchScope};
    use http_inspector_exchange::{ExchangeBuilder, ResponseFixture};
    use std::hint::black_box;
    use std::time::Instant;

    let mut app = test_app(egui::Context::default());
    for id in 0..MAX_RECORDS {
        let method = if id % 4 == 0 {
            Method::GET
        } else {
            Method::POST
        };
        let status = if id % 2 == 0 {
            StatusCode::OK
        } else {
            StatusCode::NOT_FOUND
        };
        let route = if id % 4 == 3 {
            "static/users"
        } else {
            "api/users"
        };
        let record =
            ExchangeBuilder::http(method, format!("http://api.example.test/{route}/{id}"))?
                .respond(
                    ResponseFixture::new(status)
                        .header(CONTENT_TYPE, HeaderValue::from_static("application/json")),
                )?;
        let _evicted = app.requests.records.push(record);
    }
    let context = app.context.clone();
    app.requests.search.search_now(
        (
            "method:POST url:/users status:4xx -path:/static/ res-header:content-type=json",
            SearchScope::All,
            MatchOptions::default(),
        ),
        &app.requests.records,
        &context,
    );
    let expected = MAX_RECORDS / 4;
    assert_eq!(app.requests.records.len(), MAX_RECORDS);
    assert_eq!(app.requests.filtered_records().len(), expected);
    let mut filter = Vec::new();
    for _ in 0..60 {
        let start = Instant::now();
        let rows = black_box(app.requests.filtered_records());
        filter.push(start.elapsed());
        assert_eq!(rows.len(), expected);
    }
    filter.sort_unstable();

    let expected_count = format!("{expected} of {MAX_RECORDS} requests");
    assert!(
        render_settled(&mut app, Layout::WINDOW)
            .iter()
            .any(|item| item.text == expected_count)
    );
    let frame = measure_frames(&mut app, &mut 0.0, 60);
    for (label, samples) in [("filter", filter), ("egui frame", frame)] {
        let ms = |duration: Option<&Duration>| {
            duration.map_or(f64::NAN, |duration| duration.as_secs_f64() * 1_000.0)
        };
        eprintln!(
            "{label}: 60 samples, min={:.3} ms median={:.3} ms p95={:.3} ms max={:.3} ms",
            ms(samples.first()),
            ms(samples.get(30)),
            ms(samples.get(57)),
            ms(samples.last())
        );
        assert!(
            samples
                .get(30)
                .is_some_and(|time| *time < Duration::from_millis(16))
        );
    }
    Ok(())
}

fn filter_key_event(key: egui::Key, modifiers: egui::Modifiers) -> egui::Event {
    egui::Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers,
    }
}

fn type_request_filter(app: &mut InspectorApp, text: &str) {
    let _ = render_frame(
        app,
        Layout::WINDOW,
        vec![
            filter_key_event(egui::Key::F, egui::Modifiers::COMMAND),
            egui::Event::Text(text.to_owned()),
        ],
    );
}

#[test]
fn filter_completion_leaves_tab_to_focus_navigation_inside_a_quote() {
    let mut app = test_app(egui::Context::default());
    let _ = render_settled(&mut app, Layout::WINDOW);
    let phrase = r#""connection status:""#;
    type_request_filter(&mut app, phrase);
    let _ = render_settled(&mut app, Layout::WINDOW);
    let filter_focus = app.context.memory(egui::Memory::focused);
    assert!(filter_focus.is_some());

    let (drawn, _) = render_frame(
        &mut app,
        Layout::WINDOW,
        vec![filter_key_event(
            egui::Key::ArrowLeft,
            egui::Modifiers::NONE,
        )],
    );
    assert!(drawn.iter().any(|item| item.text == phrase));
    assert!(
        !drawn.iter().any(|item| item.text == "status:2xx"),
        "field suggestions appeared inside the quoted phrase"
    );
    for key in [egui::Key::ArrowDown, egui::Key::Tab] {
        let _ = render_frame(
            &mut app,
            Layout::WINDOW,
            vec![filter_key_event(key, egui::Modifiers::NONE)],
        );
    }
    let drawn = render_settled(&mut app, Layout::WINDOW);
    assert!(drawn.iter().any(|item| item.text == phrase));
    assert_ne!(app.context.memory(egui::Memory::focused), filter_focus);
}

#[test]
fn standalone_field_completion_accepts_enter_and_tab_after_a_phrase() {
    for (arrow, accept, expected) in [
        (egui::Key::ArrowDown, egui::Key::Enter, "4xx"),
        (egui::Key::ArrowUp, egui::Key::Tab, "5xx"),
    ] {
        let mut app = test_app(egui::Context::default());
        let _ = render_settled(&mut app, Layout::WINDOW);
        let phrase = r#""한글 status:""#;
        type_request_filter(&mut app, &format!("{phrase} status:"));
        let drawn = render_settled(&mut app, Layout::WINDOW);
        assert!(drawn.iter().any(|item| item.text == "status:2xx"));

        for key in [arrow, accept] {
            let _ = render_frame(
                &mut app,
                Layout::WINDOW,
                vec![filter_key_event(key, egui::Modifiers::NONE)],
            );
        }
        let drawn = render_settled(&mut app, Layout::WINDOW);
        let input = format!("{phrase} status:{expected}");
        assert!(drawn.iter().any(|item| item.text == input), "{input}");
        assert!(!drawn.iter().any(|item| item.text == "status:2xx"));
    }
}

/// Top of the first painted text equal to `label`.
fn text_top(drawn: &[DrawnText], label: &str) -> Result<f32, String> {
    drawn
        .iter()
        .find(|item| item.text == label)
        .map(|item| item.rect.top())
        .ok_or_else(|| format!("missing {label:?}"))
}

#[tokio::test]
async fn inspector_rows_follow_the_tree_grid_in_every_tab() -> Result<(), Box<dyn std::error::Error>>
{
    let exchanges = timeout(Duration::from_secs(5), capture_sample_exchanges()).await??;
    let mut app = test_app(egui::Context::default());
    for exchange in exchanges {
        app.requests.selected = Some(exchange.id());
        let _evicted = app.requests.records.push(exchange);
    }

    app.detail.tab = DetailTab::Headers;
    let drawn = render_settled(&mut app, Layout::WINDOW);
    for group in ["Query", "Request", "Response"] {
        text_top(&drawn, group)?;
    }
    // The result line names the status and HTTP version the server sent.
    text_top(&drawn, "200 OK · HTTP/1.1")?;
    // Query parameters are one-line key-value rows, one tree row apart.
    // One-line key-value rows are one tree row apart, and their colons
    // line up in one column after the keys.
    let colons: Vec<egui::Rect> = drawn
        .iter()
        .filter(|item| item.text == ":")
        .map(|item| item.rect)
        .take(4)
        .collect();
    assert_eq!(colons.len(), 4, "query parameter rows");
    for pair in colons.windows(2) {
        if let [a, b] = pair {
            assert!(
                (b.top() - a.top() - Layout::TREE_ROW_HEIGHT).abs() < 0.5,
                "rows {a:?} and {b:?} are not {} apart",
                Layout::TREE_ROW_HEIGHT
            );
            assert!((a.left() - b.left()).abs() < 0.5);
        }
    }

    app.detail.tab = DetailTab::Timing;
    let drawn = render_settled(&mut app, Layout::WINDOW);
    let connect = text_top(&drawn, "Connect")?;
    let total = text_top(&drawn, "Total")?;
    assert!(
        5.0f32
            .mul_add(-Layout::TREE_ROW_HEIGHT, total - connect)
            .abs()
            < 1.5,
        "six timing rows: {connect} to {total}"
    );
    // Times are right-aligned in one column of the inspector (the request
    // list left of it has its own Duration column).
    let connect_left = text_left(&drawn, "Connect")?;
    let rights: Vec<f32> = drawn
        .iter()
        .filter(|item| item.text.ends_with(" ms") && item.rect.left() > connect_left)
        .map(|item| item.rect.right())
        .collect();
    assert_eq!(rights.len(), 6, "times: {rights:?}");
    assert!(
        rights.windows(2).all(|pair| match pair {
            [a, b] => (a - b).abs() < 0.5,
            _ => true,
        }),
        "times are not right-aligned: {rights:?}"
    );

    app.detail.tab = DetailTab::Body;
    let drawn = render_settled(&mut app, Layout::WINDOW);
    for label in ["Request body", "Response body", "Pretty", "Raw", "Tree"] {
        text_top(&drawn, label)?;
    }
    // The raw viewer numbers its lines.
    assert!(drawn.iter().any(|item| item.text == "1"));
    Ok(())
}

#[tokio::test]
async fn map_panes_have_headers_and_tree_rows() -> Result<(), Box<dyn std::error::Error>> {
    let exchanges = timeout(Duration::from_secs(5), capture_sample_exchanges()).await??;
    let mut app = test_app(egui::Context::default());
    load_synthetic_map(&mut app, exchanges)?;
    app.layout.main_view = MainView::Map;
    app.map.panel.sub_view = super::MapSubView::Tree;
    let drawn = render_settled(&mut app, Layout::WINDOW);
    let tree = text_top(&drawn, "Tree")?;
    let graph = text_top(&drawn, "Graph")?;
    assert!(
        (tree - graph).abs() < 2.0,
        "Tree and Graph share the header"
    );
    assert!(
        tree < Layout::TOOLBAR_HEIGHT + Layout::PANE_HEADER_HEIGHT,
        "the tabs sit in the pane header under the toolbar"
    );
    text_top(&drawn, "No node selected")?;
    // Origin rows are tree rows: consecutive roots after a collapsed one are
    // one tree row apart.
    let mut roots: Vec<f32> = drawn
        .iter()
        .filter(|item| item.text.starts_with("http://") || item.text.starts_with("https://"))
        .map(|item| item.rect.top())
        .collect();
    roots.sort_by(f32::total_cmp);
    assert_ne!(roots, Vec::<f32>::new());
    for pair in roots.windows(2) {
        if let [a, b] = pair {
            let gap = b - a;
            let rows = (gap / Layout::TREE_ROW_HEIGHT).round();
            assert!(
                rows.mul_add(-Layout::TREE_ROW_HEIGHT, gap).abs() < 1.0,
                "roots {a} and {b} are not on the {} grid",
                Layout::TREE_ROW_HEIGHT
            );
        }
    }

    app.map.panel.sub_view = super::MapSubView::Graph;
    let drawn = render_settled(&mut app, Layout::WINDOW);
    for label in ["Edges ▼", "Relayout", "Export ▼"] {
        text_top(&drawn, label)?;
    }
    Ok(())
}
