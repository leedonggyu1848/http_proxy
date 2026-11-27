//! Waterfall timeline column: shows each request's relative start time and
//! observed timing phases as horizontal bars using the same [`ExchangeTiming`]
//! data as the inspector's Timing tab.

use std::time::{Duration, Instant};

use eframe::egui::{self, Pos2, Rect, RichText, Sense, Stroke, StrokeKind};
use http_inspector_exchange::{Exchange, Outcome};

use crate::ui::i18n::Language;
use crate::ui::requests::outcome_view;
use crate::ui::style::theme::Palette;
use crate::ui::style::tokens::{Layout, Radius, Space, StrokeWidth, ThemeTokens};
use crate::ui::timing::{ExchangeTiming, TimingPhase};
use crate::ui::workbench::text;

/// Shared horizontal time range across all visible rows in the request list.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct TimelineWindow {
    origin: Instant,
    total: Duration,
}

impl TimelineWindow {
    /// Computes the timeline origin (earliest start) and total duration across
    /// `rows` as of `now`. Returns `None` when `rows` is empty.
    pub(super) fn from_exchanges(rows: &[&Exchange], now: Instant) -> Option<Self> {
        let origin = rows.iter().map(|record| record.started()).min()?;
        let total = rows
            .iter()
            .map(|record| {
                let offset = record.started().saturating_duration_since(origin);
                let end = ExchangeTiming::from_exchange(record, now).observed_end();
                offset.saturating_add(end)
            })
            .max()
            .unwrap_or_default();
        Some(Self { origin, total })
    }

    /// Returns how long after the timeline origin `started` began.
    pub(super) fn start_offset(self, started: Instant) -> Duration {
        started.saturating_duration_since(self.origin)
    }

    /// Returns the total timeline duration across all visible rows.
    pub(super) const fn total(self) -> Duration {
        self.total
    }

    /// Formats the total timeline width for the Waterfall column header, or
    /// `None` when no positive duration has been observed yet.
    pub(super) fn scale_label(self) -> Option<String> {
        format_scale(self.total)
    }
}

/// One observed phase segment normalized into `[0.0, 1.0]` along the timeline.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct BarSegment {
    pub(super) phase: TimingPhase,
    pub(super) left: f32,
    pub(super) right: f32,
}

/// Normalized geometry of one request row inside the Waterfall column.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct WaterfallLayout {
    pub(super) start: f32,
    pub(super) end: f32,
    /// Observed non-zero phase segments only. Unobserved phases are omitted so
    /// their intervals remain blank rather than zero-width bars.
    pub(super) segments: Vec<BarSegment>,
    /// Unobserved intervals within `[start, end]` not covered by any segment.
    pub(super) gaps: Vec<(f32, f32)>,
}

impl WaterfallLayout {
    /// Maps `start_offset` and `timing` into `[0.0, 1.0]` within `total_window`.
    pub(super) fn compute(
        start_offset: Duration,
        timing: &ExchangeTiming,
        total_window: Duration,
    ) -> Self {
        let total_secs = total_window.as_secs_f64();
        if total_secs <= 0.0 {
            return Self {
                start: 0.0,
                end: 0.0,
                segments: Vec::new(),
                gaps: Vec::new(),
            };
        }
        let start = fraction(start_offset, total_secs);
        let end = fraction(
            start_offset.saturating_add(timing.observed_end()),
            total_secs,
        )
        .max(start);

        let mut segments = Vec::with_capacity(TimingPhase::ALL.len());
        for phase in TimingPhase::ALL {
            let Some(span) = timing.span(phase) else {
                continue;
            };
            if span.duration().is_zero() {
                continue;
            }
            let left = fraction(start_offset.saturating_add(span.start()), total_secs);
            let right = fraction(start_offset.saturating_add(span.end()), total_secs).max(left);
            segments.push(BarSegment { phase, left, right });
        }

        let gaps = compute_gaps(start, end, &segments);
        Self {
            start,
            end,
            segments,
            gaps,
        }
    }
}

/// Computes the sub-intervals of `[start, end]` not covered by any observed
/// segment, leaving unobserved phases explicitly blank.
fn compute_gaps(start: f32, end: f32, segments: &[BarSegment]) -> Vec<(f32, f32)> {
    const EPS: f32 = 1e-4;
    if end - start <= EPS {
        return Vec::new();
    }
    let mut covered: Vec<(f32, f32)> = segments
        .iter()
        .map(|seg| (seg.left.clamp(start, end), seg.right.clamp(start, end)))
        .filter(|(left, right)| right - left > EPS)
        .collect();
    covered.sort_by(|left, right| left.0.total_cmp(&right.0));

    let mut gaps = Vec::new();
    let mut cursor = start;
    for (left, right) in covered {
        if left - cursor > EPS {
            gaps.push((cursor, left));
        }
        cursor = cursor.max(right);
    }
    if end - cursor > EPS {
        gaps.push((cursor, end));
    }
    gaps
}

#[expect(
    clippy::cast_possible_truncation,
    reason = "normalized fractions lie in [0.0, 1.0], where f32 precision exceeds pixel resolution"
)]
fn fraction(duration: Duration, total_secs: f64) -> f32 {
    (duration.as_secs_f64() / total_secs).clamp(0.0, 1.0) as f32
}

/// Formats the timeline scale label for the Waterfall column header.
pub(super) fn format_scale(total: Duration) -> Option<String> {
    if total.is_zero() {
        return None;
    }
    let millis = outcome_view::elapsed_millis(total);
    if millis < 10.0 {
        Some(format!("{millis:.1} ms"))
    } else if millis < 1_000.0 {
        Some(format!("{millis:.0} ms"))
    } else {
        let secs = total.as_secs_f64();
        if secs < 10.0 {
            Some(format!("{secs:.2} s"))
        } else {
            Some(format!("{secs:.1} s"))
        }
    }
}

/// Paints one Waterfall cell with observed phase bars and blank gaps for
/// unobserved intervals. The caller attaches [`attach_hover`] to the table
/// cell's `Response` so hover, row click selection, and context menus share one
/// interaction target.
pub(super) fn paint_cell_ui(
    ui: &mut egui::Ui,
    record: &Exchange,
    timeline: Option<TimelineWindow>,
    now: Instant,
) {
    let available = egui::vec2(ui.available_width().max(1.0), Layout::TABLE_ROW_HEIGHT);
    let (rect, _) = ui.allocate_exact_size(available, Sense::empty());
    let Some(timeline) = timeline else {
        return;
    };
    if ui.is_rect_visible(rect) {
        let start_offset = timeline.start_offset(record.started());
        let timing = ExchangeTiming::from_exchange(record, now);
        let layout = WaterfallLayout::compute(start_offset, &timing, timeline.total());
        let tokens = ThemeTokens::for_theme(ui.ctx().theme());
        paint_cell(ui.painter(), rect, &layout, timing.is_running(), &tokens);
    }
}

/// Attaches the timing breakdown hover table to the Waterfall cell's response.
pub(super) fn attach_hover(
    cell: egui::Response,
    record: &Exchange,
    timeline: Option<TimelineWindow>,
    now: Instant,
    lang: Language,
) -> egui::Response {
    let Some(timeline) = timeline else {
        return cell;
    };
    cell.on_hover_ui(|ui| {
        let start_offset = timeline.start_offset(record.started());
        let timing = ExchangeTiming::from_exchange(record, now);
        hover_table(ui, start_offset, &timing, record.outcome(), lang);
    })
}

fn paint_cell(
    painter: &egui::Painter,
    rect: Rect,
    layout: &WaterfallLayout,
    running: bool,
    tokens: &ThemeTokens,
) {
    let track = Rect::from_min_max(
        Pos2::new(rect.left() + Space::XS, rect.top()),
        Pos2::new(
            (rect.right() - Space::XS).max(rect.left() + Space::XS),
            rect.bottom(),
        ),
    );
    if track.width() <= 1.0 {
        return;
    }
    let cy = track.center().y;
    paint_grid(painter, track, tokens);

    for &(gap_left, gap_right) in &layout.gaps {
        let x0 = track_x(track, gap_left);
        let x1 = track_x(track, gap_right);
        paint_dashed_gap(painter, x0, x1, cy, tokens);
    }

    let mut has_bar_at_start = false;
    let mut has_bar_at_end = false;
    for segment in &layout.segments {
        let (x0, x1) = segment_pixel_range(track, segment.left, segment.right);
        paint_segment(painter, x0, x1, cy, segment.phase, tokens);
        if (segment.left - layout.start).abs() < 1e-3 {
            has_bar_at_start = true;
        }
        if (segment.right - layout.end).abs() < 1e-3 {
            has_bar_at_end = true;
        }
    }

    if !has_bar_at_start {
        paint_tick(painter, track_x(track, layout.start), cy, tokens);
    }
    let has_extent = (layout.end - layout.start) > 1e-3;
    if has_extent && !has_bar_at_end && !running {
        paint_tick(painter, track_x(track, layout.end), cy, tokens);
    }
}

fn track_x(track: Rect, frac: f32) -> f32 {
    frac.clamp(0.0, 1.0).mul_add(track.width(), track.left())
}

/// Keeps observed non-zero phases at least `WATERFALL_MIN_BAR_WIDTH` wide
/// without overflowing the track's right boundary.
fn segment_pixel_range(track: Rect, left: f32, right: f32) -> (f32, f32) {
    let min_width = Layout::WATERFALL_MIN_BAR_WIDTH.min(track.width());
    let mut x0 = track_x(track, left);
    let mut x1 = track_x(track, right).max(x0 + min_width);
    if x1 > track.right() {
        x1 = track.right();
        x0 = (x1 - min_width).max(track.left());
    }
    (x0, x1)
}

/// Subtle quarter-step vertical reference lines so rows align visually (D19).
fn paint_grid(painter: &egui::Painter, track: Rect, tokens: &ThemeTokens) {
    let stroke = Stroke::new(StrokeWidth::STRUCTURAL, tokens.borders.subtle);
    for quarter in [0.25_f32, 0.50, 0.75] {
        let x = track_x(track, quarter).round() + 0.5;
        painter.vline(x, track.y_range(), stroke);
    }
}

/// Draws a dashed baseline through an unobserved interval so it reads as an
/// empty gap while meeting the 3:1 non-text contrast requirement on every row
/// surface (D7).
fn paint_dashed_gap(painter: &egui::Painter, x0: f32, x1: f32, cy: f32, tokens: &ThemeTokens) {
    let span = x1 - x0;
    if span <= StrokeWidth::STRUCTURAL {
        return;
    }
    let stroke = Stroke::new(StrokeWidth::STRUCTURAL, tokens.text.secondary);
    let stride = Layout::GRAPH_DASH * 2.0;
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "positive dash count along a finite screen-width track"
    )]
    let steps = (span / stride).ceil() as usize;
    for step in 0..steps {
        #[expect(
            clippy::cast_precision_loss,
            reason = "dash index along a finite screen-width track"
        )]
        let dash_start = (step as f32).mul_add(stride, x0);
        let dash_end = (dash_start + Layout::GRAPH_DASH).min(x1);
        painter.line_segment([Pos2::new(dash_start, cy), Pos2::new(dash_end, cy)], stroke);
    }
}

/// Marks the start or end boundary of a request when no observed phase bar
/// touches that boundary.
fn paint_tick(painter: &egui::Painter, x: f32, cy: f32, tokens: &ThemeTokens) {
    let half = Layout::WATERFALL_TICK_HEIGHT / 2.0;
    painter.vline(
        x,
        (cy - half)..=(cy + half),
        Stroke::new(StrokeWidth::STRUCTURAL, tokens.text.secondary),
    );
}

/// Paints one observed phase bar. Each phase differs in shape and/or contrast-
/// verified token color so color alone never carries meaning (D1, D3, D7, D9).
fn paint_segment(
    painter: &egui::Painter,
    x0: f32,
    x1: f32,
    cy: f32,
    phase: TimingPhase,
    tokens: &ThemeTokens,
) {
    match phase {
        TimingPhase::Connect => {
            let half = Layout::WATERFALL_HOLLOW_HEIGHT / 2.0;
            let bar = Rect::from_min_max(Pos2::new(x0, cy - half), Pos2::new(x1, cy + half));
            painter.rect_stroke(
                bar,
                Radius::PANE,
                Stroke::new(StrokeWidth::STRUCTURAL, tokens.text.secondary),
                StrokeKind::Inside,
            );
        }
        TimingPhase::Send => {
            let half = Layout::WATERFALL_THIN_HEIGHT / 2.0;
            let bar = Rect::from_min_max(Pos2::new(x0, cy - half), Pos2::new(x1, cy + half));
            painter.rect_filled(bar, Radius::PANE, tokens.text.secondary);
        }
        TimingPhase::Wait => {
            let half = Layout::WATERFALL_BAR_HEIGHT / 2.0;
            let bar = Rect::from_min_max(Pos2::new(x0, cy - half), Pos2::new(x1, cy + half));
            painter.rect_filled(bar, Radius::PANE, tokens.accent.primary);
        }
        TimingPhase::Receive => {
            let half = Layout::WATERFALL_BAR_HEIGHT / 2.0;
            let bar = Rect::from_min_max(Pos2::new(x0, cy - half), Pos2::new(x1, cy + half));
            painter.rect_filled(bar, Radius::PANE, tokens.text.primary);
        }
    }
}

/// Hover breakdown showing the start offset and the exact six entries of the
/// inspector's Timing tab alongside each phase's bar sample (S1, S18).
fn hover_table(
    ui: &mut egui::Ui,
    start_offset: Duration,
    timing: &ExchangeTiming,
    outcome: &Outcome,
    lang: Language,
) {
    let tokens = ThemeTokens::for_theme(ui.ctx().theme());
    let palette = Palette::of(ui);
    let entries = timing.entries(outcome, lang);
    let start_value = format!(
        "+{}",
        text::milliseconds(outcome_view::elapsed_millis(start_offset))
    );

    egui::Grid::new("waterfall-hover")
        .striped(false)
        .num_columns(2)
        .spacing(egui::vec2(Space::MD, Space::XS))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = Space::S;
                paint_legend_sample(ui, None, &tokens);
                ui.label(RichText::new(lang.requests().start_time).color(palette.secondary));
            });
            hover_value(ui, &start_value, palette.text);
            ui.end_row();

            for entry in &entries {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = Space::S;
                    paint_legend_sample(ui, entry.phase, &tokens);
                    ui.label(RichText::new(entry.label).color(palette.secondary));
                });
                let value = entry.value_text(lang);
                let color = if entry.millis.is_some() {
                    palette.text
                } else {
                    palette.secondary
                };
                hover_value(ui, &value, color);
                ui.end_row();
            }
        });
}

fn hover_value(ui: &mut egui::Ui, value: &str, color: egui::Color32) {
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        ui.label(RichText::new(value).monospace().color(color));
    });
}

fn paint_legend_sample(ui: &mut egui::Ui, phase: Option<TimingPhase>, tokens: &ThemeTokens) {
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(Layout::WATERFALL_LEGEND_WIDTH, Layout::WATERFALL_BAR_HEIGHT),
        Sense::hover(),
    );
    if let Some(phase) = phase
        && ui.is_rect_visible(rect)
    {
        paint_segment(
            ui.painter(),
            rect.left(),
            rect.right(),
            rect.center().y,
            phase,
            tokens,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use http::{Method, StatusCode};
    use http_inspector_exchange::{
        BodyLifecycle, ExchangeBuilder, FailureStage, PhaseSpan, ResponseFixture,
    };

    fn approx(left: f32, right: f32) -> bool {
        (left - right).abs() < 1e-4
    }

    #[test]
    fn consecutive_observed_phases_fill_bar_without_gaps() {
        let timing = ExchangeTiming::from_parts(
            Some(PhaseSpan::new(Duration::ZERO, Duration::from_millis(10))),
            Some(PhaseSpan::new(
                Duration::from_millis(10),
                Duration::from_millis(5),
            )),
            Some(PhaseSpan::new(
                Duration::from_millis(15),
                Duration::from_millis(25),
            )),
            Some(PhaseSpan::new(
                Duration::from_millis(40),
                Duration::from_millis(10),
            )),
            Some(Duration::from_millis(40)),
            Some(Duration::from_millis(50)),
            false,
            None,
        );
        let layout = WaterfallLayout::compute(
            Duration::from_millis(25),
            &timing,
            Duration::from_millis(100),
        );
        assert!(approx(layout.start, 0.25));
        assert!(approx(layout.end, 0.75));
        assert_eq!(layout.segments.len(), 4);
        assert!(
            layout.gaps.is_empty(),
            "expected no gaps: {:?}",
            layout.gaps
        );
        assert_eq!(
            layout.segments.first().map(|s| s.phase),
            Some(TimingPhase::Connect)
        );
        assert!(approx(
            layout.segments.first().map_or(0.0, |s| s.left),
            0.25
        ));
        assert!(approx(
            layout.segments.first().map_or(0.0, |s| s.right),
            0.35
        ));
        assert_eq!(
            layout.segments.get(2).map(|s| s.phase),
            Some(TimingPhase::Wait)
        );
        assert!(approx(layout.segments.get(2).map_or(0.0, |s| s.left), 0.40));
        assert!(approx(
            layout.segments.get(2).map_or(0.0, |s| s.right),
            0.65
        ));
        assert_eq!(
            layout.segments.get(3).map(|s| s.phase),
            Some(TimingPhase::Receive)
        );
        assert!(approx(layout.segments.get(3).map_or(0.0, |s| s.left), 0.65));
        assert!(approx(
            layout.segments.get(3).map_or(0.0, |s| s.right),
            0.75
        ));
    }

    #[test]
    fn unobserved_phases_produce_blank_gaps_instead_of_zero_bars() {
        // Connect missing, gap before Send, Wait missing, Receive observed.
        let timing = ExchangeTiming::from_parts(
            None,
            Some(PhaseSpan::new(
                Duration::from_millis(10),
                Duration::from_millis(10),
            )),
            None,
            Some(PhaseSpan::new(
                Duration::from_millis(40),
                Duration::from_millis(10),
            )),
            Some(Duration::from_millis(40)),
            Some(Duration::from_millis(50)),
            false,
            None,
        );
        let layout = WaterfallLayout::compute(Duration::ZERO, &timing, Duration::from_millis(100));
        assert_eq!(
            layout.segments.iter().map(|s| s.phase).collect::<Vec<_>>(),
            vec![TimingPhase::Send, TimingPhase::Receive]
        );
        assert_eq!(layout.gaps.len(), 2);
        assert!(approx(layout.gaps.first().map_or(-1.0, |g| g.0), 0.0));
        assert!(approx(layout.gaps.first().map_or(-1.0, |g| g.1), 0.1));
        assert!(approx(layout.gaps.get(1).map_or(-1.0, |g| g.0), 0.2));
        assert!(approx(layout.gaps.get(1).map_or(-1.0, |g| g.1), 0.4));
    }

    #[test]
    fn failed_or_in_flight_request_with_no_observed_phases_renders_as_single_gap() {
        let timing = ExchangeTiming::from_parts(
            None,
            None,
            None,
            None,
            Some(Duration::from_millis(30)),
            Some(Duration::from_millis(30)),
            false,
            None,
        );
        let layout = WaterfallLayout::compute(
            Duration::from_millis(20),
            &timing,
            Duration::from_millis(100),
        );
        assert_eq!(layout.segments, []);
        assert_eq!(layout.gaps.len(), 1);
        assert!(approx(layout.gaps.first().map_or(-1.0, |g| g.0), 0.20));
        assert!(approx(layout.gaps.first().map_or(-1.0, |g| g.1), 0.50));
    }

    #[test]
    fn handles_zero_total_window_and_zero_duration_phases_safely() {
        let timing = ExchangeTiming::from_parts(
            Some(PhaseSpan::new(Duration::ZERO, Duration::ZERO)),
            Some(PhaseSpan::new(Duration::ZERO, Duration::ZERO)),
            Some(PhaseSpan::new(Duration::ZERO, Duration::ZERO)),
            Some(PhaseSpan::new(Duration::ZERO, Duration::ZERO)),
            Some(Duration::ZERO),
            Some(Duration::ZERO),
            false,
            None,
        );
        let zero_window = WaterfallLayout::compute(Duration::ZERO, &timing, Duration::ZERO);
        assert!(approx(zero_window.start, 0.0));
        assert!(approx(zero_window.end, 0.0));
        assert_eq!(zero_window.segments, []);
        assert_eq!(zero_window.gaps, []);

        let positive_window = WaterfallLayout::compute(
            Duration::from_millis(10),
            &timing,
            Duration::from_millis(50),
        );
        assert!(approx(positive_window.start, 0.2));
        assert!(approx(positive_window.end, 0.2));
        assert_eq!(positive_window.segments, []);
        assert_eq!(positive_window.gaps, []);
    }

    #[test]
    fn timeline_window_is_order_independent_and_uses_earliest_start()
    -> Result<(), Box<dyn std::error::Error>> {
        let first = ExchangeBuilder::http(Method::GET, "http://example.test/1")?
            .connect_time(Duration::from_millis(5))
            .respond(ResponseFixture::new(StatusCode::OK))?;
        let second = ExchangeBuilder::http(Method::GET, "http://example.test/2")?
            .fail(FailureStage::Connect, "refused")?;
        let third = ExchangeBuilder::http(Method::GET, "http://example.test/3")?
            .respond(ResponseFixture::new(StatusCode::OK).body("part", BodyLifecycle::Streaming))?;
        let now = third.started() + Duration::from_millis(40);

        let forward = TimelineWindow::from_exchanges(&[&first, &second, &third], now)
            .ok_or("missing timeline")?;
        let reversed = TimelineWindow::from_exchanges(&[&third, &second, &first], now)
            .ok_or("missing reversed timeline")?;
        assert_eq!(forward, reversed);
        assert_eq!(forward.start_offset(first.started()), Duration::ZERO);
        assert!(forward.total() >= Duration::from_millis(40));
        assert_eq!(TimelineWindow::from_exchanges(&[], now), None);
        Ok(())
    }

    #[test]
    fn formats_scale_across_millisecond_and_second_ranges() {
        assert_eq!(format_scale(Duration::ZERO), None);
        assert_eq!(
            format_scale(Duration::from_micros(2_500)),
            Some("2.5 ms".to_owned())
        );
        assert_eq!(
            format_scale(Duration::from_millis(145)),
            Some("145 ms".to_owned())
        );
        assert_eq!(
            format_scale(Duration::from_millis(1_250)),
            Some("1.25 s".to_owned())
        );
        assert_eq!(
            format_scale(Duration::from_millis(12_340)),
            Some("12.3 s".to_owned())
        );
    }

    #[test]
    fn segment_pixel_range_clamps_subpixel_bars_inside_track() {
        let track = Rect::from_min_max(Pos2::new(10.0, 0.0), Pos2::new(110.0, 26.0));
        let (x0, x1) = segment_pixel_range(track, 0.5, 0.5001);
        assert!((x0 - 60.0).abs() < 1e-3);
        assert!((x1 - 62.0).abs() < 1e-3);

        let (end_x0, end_x1) = segment_pixel_range(track, 0.999, 1.0);
        assert!((end_x1 - 110.0).abs() < 1e-3);
        assert!((end_x0 - 108.0).abs() < 1e-3);
    }

    #[test]
    fn unobserved_dashed_gap_uses_contrast_verified_secondary_color() {
        fn collect_horizontal_lines(shape: &egui::Shape, out: &mut Vec<egui::Color32>) {
            match shape {
                egui::Shape::LineSegment { points, stroke }
                    if (points[0].y - points[1].y).abs() < 1e-3
                        && (points[1].x - points[0].x).abs() > 0.5 =>
                {
                    out.push(stroke.color);
                }
                egui::Shape::Vec(items) => {
                    for item in items {
                        collect_horizontal_lines(item, out);
                    }
                }
                _ => {}
            }
        }

        let timing = ExchangeTiming::from_parts(
            None,
            None,
            None,
            None,
            Some(Duration::from_millis(60)),
            Some(Duration::from_millis(60)),
            false,
            None,
        );
        let layout = WaterfallLayout::compute(
            Duration::from_millis(10),
            &timing,
            Duration::from_millis(100),
        );

        for theme in [egui::Theme::Dark, egui::Theme::Light] {
            let tokens = ThemeTokens::for_theme(theme);
            let ctx = egui::Context::default();
            let viewport =
                Rect::from_min_size(Pos2::ZERO, egui::vec2(120.0, Layout::TABLE_ROW_HEIGHT));
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(viewport),
                    ..Default::default()
                },
                |ui| {
                    egui::CentralPanel::default()
                        .frame(egui::Frame::NONE)
                        .show(ui, |ui| {
                            paint_cell(ui.painter(), viewport, &layout, false, &tokens);
                        });
                },
            );
            output.textures_delta.clear();

            let mut dash_colors = Vec::new();
            for clipped in &output.shapes {
                collect_horizontal_lines(&clipped.shape, &mut dash_colors);
            }
            assert!(
                !dash_colors.is_empty(),
                "expected dashed gap segments for {theme:?}"
            );
            assert!(
                dash_colors
                    .iter()
                    .all(|&color| color == tokens.text.secondary),
                "expected all dashed gap segments to use tokens.text.secondary for {theme:?}, got {dash_colors:?}"
            );
        }
    }
}
