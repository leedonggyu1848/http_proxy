//! Decoded, formatted, and highlighted body text for the detail panel,
//! rebuilt only when the shown body or the way it is shown changes.

use crate::ui::detail::body_matches::{BodyMatches, Hit};
use crate::ui::detail::highlight;
use crate::ui::detail::search::Query;
use crate::ui::detail::search::highlight::MatchColors;
use crate::ui::detail::tree_view::TreeView;
use crate::ui::style::theme::{Palette, Theme};
use eframe::egui::{self, Galley, text::LayoutJob};
use http_proxy::content::decode::{self as body_view, BodyText};
use http_proxy::content::detect;
use http_proxy::content::format::{
    self as body_format, BodyFormat, FormatError, Notice, NoticeLevel, ViewMode, tree,
};
use http_proxy::exchange::{BodyLifecycle, BodyRetention, MediaType};
use std::sync::Arc;

/// Height of the numbered line view before it scrolls.
const LINE_VIEW_HEIGHT: f32 = 480.0;

/// Which body of an exchange is shown; both can be on screen at once.
pub(in crate::ui) use http_proxy::exchange::BodySide;

/// A captured body preview to show.
pub(in crate::ui) struct BodySource<'a> {
    /// [`http_proxy::exchange::RecordId::value`] of the exchange the body belongs to.
    pub(in crate::ui) record: u64,
    pub(in crate::ui) side: BodySide,
    pub(in crate::ui) bytes: &'a [u8],
    pub(in crate::ui) encodings: &'a [&'a str],
    pub(in crate::ui) media_type: Option<&'a MediaType>,
    /// How much of the body the retained bytes cover.
    pub(in crate::ui) capture: Capture,
    /// The search whose matches are marked, when it covers this body.
    pub(in crate::ui) search: Option<&'a Query>,
}

/// How much of a body the retained bytes cover, and why not all of it.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(in crate::ui) enum Capture {
    /// The retained bytes are the whole body.
    Whole,
    /// The body was longer than the preview keeps.
    Truncated,
    /// More of the body may still arrive.
    Streaming,
    /// The body stopped before its end: the sender interrupted it or the
    /// receiver cancelled it.
    Ended,
}

impl Capture {
    /// Reads the capture state of a body snapshot. A preview cut at the
    /// retention limit stays cut whatever happens to the stream, so
    /// truncation wins over the lifecycle.
    pub(in crate::ui) const fn of(lifecycle: BodyLifecycle, retention: BodyRetention) -> Self {
        match (lifecycle, retention) {
            (_, BodyRetention::Truncated | BodyRetention::Omitted) => Self::Truncated,
            (BodyLifecycle::Complete, BodyRetention::Prefix) => Self::Whole,
            (BodyLifecycle::Streaming, BodyRetention::Prefix) => Self::Streaming,
            (BodyLifecycle::Interrupted | BodyLifecycle::Cancelled, BodyRetention::Prefix) => {
                Self::Ended
            }
        }
    }

    const fn is_whole(self) -> bool {
        matches!(self, Self::Whole)
    }

    /// Why the view ends before the body does; `None` for a whole body.
    fn notice(self) -> Option<Notice> {
        let text = match self {
            Self::Whole => return None,
            Self::Truncated => format!(
                "Only part of the body was captured (at most {} KiB is kept); the view ends where the capture ends.",
                http_proxy::interception::BODY_PREVIEW_BYTES / 1024
            ),
            Self::Streaming => {
                "The body is still being received; the view shows what has arrived so far."
                    .to_owned()
            }
            Self::Ended => {
                "The body ended early (interrupted or cancelled); the view shows what was received."
                    .to_owned()
            }
        };
        Some(Notice::warning(text))
    }
}

/// Everything the displayed text depends on. Retained previews only grow,
/// so their length identifies the bytes of one record's body.
#[derive(Clone, Debug, PartialEq)]
struct Key {
    record: u64,
    side: BodySide,
    retained: usize,
    capture: Capture,
    mode: ViewMode,
    theme: Theme,
    font: egui::FontId,
    /// [`Query::generation`] of the marked search.
    search: Option<u64>,
}

enum Content {
    Unavailable(String),
    /// The side's [`TreeCache`] holds the tree to show.
    Tree {
        format: BodyFormat,
        notices: Vec<Notice>,
    },
    Ready {
        format: BodyFormat,
        /// Shown above the text in order.
        notices: Vec<Notice>,
        job: LayoutJob,
        /// One layout per line for formats shown with line numbers.
        lines: Option<Vec<LayoutJob>>,
        /// Search matches, with `job` and `lines` before they were marked.
        matches: Option<Box<Marked>>,
        /// The last laid-out galley and the wrap width it was laid out for.
        galley: Option<(u32, Arc<Galley>)>,
    },
}

/// Search matches in a body view and the text before marking, so moving to
/// another match marks it again from the original.
struct Marked {
    matches: BodyMatches,
    job: LayoutJob,
    lines: Option<Vec<LayoutJob>>,
}

/// The tree of one side's body with its open rows and search. It is kept
/// across view mode, theme, and font changes and rebuilt only for other
/// bytes: a different record or a longer retained preview.
#[derive(Default)]
struct TreeCache {
    entry: Option<TreeEntry>,
    #[cfg(test)]
    builds: u64,
}

struct TreeEntry {
    record: u64,
    retained: usize,
    capture: Capture,
    tree: Result<TreeView, FormatError>,
}

impl TreeCache {
    /// The tree of `text`, read as `format`; `None` when the format has
    /// none, and the reason when `text` could not be read as one.
    fn get(
        &mut self,
        source: &BodySource<'_>,
        format: BodyFormat,
        text: &str,
    ) -> Option<Result<&TreeView, &FormatError>> {
        let current = self.entry.as_ref().is_some_and(|entry| {
            (entry.record, entry.retained, entry.capture)
                == (source.record, source.bytes.len(), source.capture)
        });
        if !current {
            let tree = tree::build(format, text)?;
            #[cfg(test)]
            {
                self.builds += 1;
            }
            self.entry = Some(TreeEntry {
                record: source.record,
                retained: source.bytes.len(),
                capture: source.capture,
                tree: tree.map(TreeView::new),
            });
        }
        let entry = self.entry.as_ref()?;
        Some(entry.tree.as_ref())
    }

    fn view(&mut self) -> Option<&mut TreeView> {
        self.entry.as_mut()?.tree.as_mut().ok()
    }
}

/// What one side showed last.
#[derive(Default)]
struct SideCache {
    shown: Option<(Key, Content)>,
    tree: TreeCache,
}

/// Highlighted text of the request and response bodies last shown.
#[derive(Default)]
pub(in crate::ui) struct BodyViewCache {
    request: SideCache,
    response: SideCache,
    builds: u64,
}

impl BodyViewCache {
    /// Shows `source` in `mode`, decoding and highlighting it only when it
    /// differs from what this side showed last.
    pub(in crate::ui) fn show(
        &mut self,
        ui: &mut egui::Ui,
        source: &BodySource<'_>,
        mode: ViewMode,
    ) {
        let key = Key {
            record: source.record,
            side: source.side,
            retained: source.bytes.len(),
            capture: source.capture,
            mode,
            theme: ui.ctx().theme(),
            font: egui::TextStyle::Monospace.resolve(ui.style()),
            search: source.search.map(|query| query.generation),
        };
        let side = match source.side {
            BodySide::Request => &mut self.request,
            BodySide::Response => &mut self.response,
        };
        if side.shown.as_ref().is_none_or(|(cached, _)| *cached != key) {
            if side
                .shown
                .as_ref()
                .is_some_and(|(cached, _)| cached.record != key.record || cached.mode != key.mode)
            {
                reset_text_selection(ui, key.side);
            }
            self.builds += 1;
            // Use the keyed theme's style: after a theme switch this frame's
            // style still belongs to the previous theme.
            let text_color = ui.ctx().style_of(key.theme).visuals.text_color();
            let content = build(ui.ctx(), source, &key, text_color, &mut side.tree);
            side.shown = Some((key, content));
        }
        if let Some((key, content)) = &mut side.shown {
            render(ui, content, key, &mut side.tree);
        }
    }

    #[cfg(test)]
    const fn builds(&self) -> u64 {
        self.builds
    }
}

fn build(
    ctx: &egui::Context,
    source: &BodySource<'_>,
    key: &Key,
    text_color: egui::Color32,
    trees: &mut TreeCache,
) -> Content {
    let (text, decoding) =
        match body_view::decode(source.bytes, source.encodings, source.capture.is_whole()) {
            BodyText::Ready { text, notice } => (text, notice),
            BodyText::Unavailable(message) => return Content::Unavailable(message),
        };
    let format = detect::settle(source.media_type, text.as_bytes());
    let mut notices = capture_notices(decoding, source.capture);
    if key.mode == ViewMode::Tree {
        match trees.get(source, format, &text) {
            Some(Ok(view)) => {
                let incomplete = view.incomplete();
                notices.extend(incomplete.map(Notice::warning));
                if source.search.is_some() {
                    notices.push(Notice::info("Search matches are marked in Pretty and Raw."));
                }
                return Content::Tree { format, notices };
            }
            Some(Err(reason)) => notices.push(Notice::warning(format!(
                "Showing Pretty: could not read a tree: {reason}."
            ))),
            None => notices.push(Notice::warning(format!(
                "Tree view is not available for {}; showing Pretty.",
                format.label()
            ))),
        }
    }
    let formatted = body_format::format(format, key.mode, &text);
    notices.extend(formatted.notices);
    let palette = highlight::SyntaxPalette::for_theme(key.theme);
    let job = highlight::layout_job(ctx, &formatted.styled, &palette, text_color, &key.font);
    let lines = format
        .spec()
        .numbered
        .then(|| highlight::line_jobs(ctx, &formatted.styled, &palette, text_color, &key.font));
    let Some(query) = source.search else {
        return Content::Ready {
            format,
            notices,
            job,
            lines,
            matches: None,
            galley: None,
        };
    };
    let matches = BodyMatches::find(
        &query.matcher,
        &job,
        lines.as_deref(),
        MatchColors::for_theme(key.theme),
    );
    let (marked_job, marked_lines) = matches.mark(&job, lines.as_deref());
    Content::Ready {
        format,
        notices,
        job: marked_job,
        lines: marked_lines,
        matches: Some(Box::new(Marked {
            matches,
            job,
            lines,
        })),
        galley: None,
    }
}

/// What every format says about how the body was captured: the decoding
/// used, and why the view ends early when only part of the body was kept.
fn capture_notices(decoding: Option<String>, capture: Capture) -> Vec<Notice> {
    decoding
        .map(Notice::info)
        .into_iter()
        .chain(capture.notice())
        .collect()
}

fn render(ui: &mut egui::Ui, content: &mut Content, key: &Key, trees: &mut TreeCache) {
    let (format, notices, job, lines, matches, galley) = match content {
        Content::Unavailable(message) => {
            ui.colored_label(Palette::of(ui).warning, message.as_str());
            return;
        }
        Content::Tree { format, notices } => {
            render_header(ui, *format, notices, key);
            if let Some(view) = trees.view() {
                ui.push_id(("body-tree", key.side), |ui| {
                    view.show(ui, key.theme, &key.font);
                });
            }
            return;
        }
        Content::Ready {
            format,
            notices,
            job,
            lines,
            matches,
            galley,
        } => (*format, &*notices, job, lines, matches, galley),
    };
    render_header(ui, format, notices, key);
    let scroll = matches.as_mut().and_then(|marked| {
        if marked.matches.controls(ui) {
            (*job, *lines) = marked.matches.mark(&marked.job, marked.lines.as_deref());
            *galley = None;
        }
        marked.matches.take_scroll()
    });
    if let Some(lines) = lines {
        render_numbered_lines(ui, lines, key.side, scroll.map(|hit| hit.line));
        return;
    }
    let mut text = job.text.as_str();
    let mut layouter = |ui: &egui::Ui, _: &dyn egui::TextBuffer, wrap_width: f32| {
        let width = wrap_width.to_bits();
        match galley {
            Some((cached, galley)) if *cached == width => Arc::clone(galley),
            _ => {
                let mut wrapped = job.clone();
                wrapped.wrap.max_width = wrap_width;
                let has_indent = clamp_leading_space(&mut wrapped, wrap_width);
                let mut laid_out = ui
                    .ctx()
                    .fonts_mut(|fonts| fonts.layout_job(wrapped.clone()));
                if has_indent && dropped_leading_indent(&laid_out) {
                    wrapped.wrap.break_anywhere = true;
                    laid_out = ui.ctx().fonts_mut(|fonts| fonts.layout_job(wrapped));
                }
                *galley = Some((width, Arc::clone(&laid_out)));
                laid_out
            }
        }
    };
    let output = egui::TextEdit::multiline(&mut text)
        .id(body_text_id(ui, key.side))
        .font(egui::TextStyle::Monospace)
        .desired_rows(8)
        .desired_width(f32::INFINITY)
        .layouter(&mut layouter)
        .show(ui);
    if let Some(hit) = scroll {
        scroll_to_hit(ui, &output, &job.text, &hit);
    }
}

/// Persistent widget id for one side's non-numbered read-only body text view.
fn body_text_id(ui: &egui::Ui, side: BodySide) -> egui::Id {
    ui.make_persistent_id(("body-text", side))
}

/// Clears cached selection state and focus for `side` so switching records or
/// view modes never carries stale cursor offsets into different text.
fn reset_text_selection(ui: &egui::Ui, side: BodySide) {
    let id = body_text_id(ui, side);
    ui.ctx().data_mut(|data| {
        data.remove::<egui::text_edit::TextEditState>(id);
    });
    ui.memory_mut(|memory| {
        if memory.has_focus(id) {
            memory.surrender_focus(id);
        }
    });
}

/// Bounds visual continuation indents on narrow viewports so long prefixes
/// never squeeze continuation lines into unreadable narrow columns.
/// Returns `true` when at least one section keeps a non-zero visual indent.
fn clamp_leading_space(job: &mut LayoutJob, wrap_width: f32) -> bool {
    let max_indent = if wrap_width.is_finite() {
        Some((wrap_width * 0.5).min((wrap_width - 80.0).max(0.0)))
    } else {
        None
    };
    let mut has_indent = false;
    for section in &mut job.sections {
        if let Some(limit) = max_indent
            && section.leading_space > limit
        {
            section.leading_space = limit;
        }
        if section.leading_space > 0.0 {
            has_indent = true;
        }
    }
    has_indent
}

/// Detects epaint's fallback that inserts a non-final zero-height empty row and
/// resets `leading_space` to zero when a single unbreakable token exceeds the
/// space remaining after the indent. Normal empty rows at a trailing `\n` are
/// final and have `row.size().y > 0.0`.
fn dropped_leading_indent(galley: &Galley) -> bool {
    let check_len = galley.rows.len().saturating_sub(1);
    galley
        .rows
        .iter()
        .take(check_len)
        .any(|row| !row.ends_with_newline && row.glyphs.is_empty() && row.size.y == 0.0)
}

/// Scrolls the enclosing scroll area so the match `hit` in `text` is in view.
fn scroll_to_hit(ui: &egui::Ui, output: &egui::text_edit::TextEditOutput, text: &str, hit: &Hit) {
    let characters = text
        .get(..hit.range.start)
        .map_or(0, |prefix| prefix.chars().count());
    let rect = output
        .galley
        .pos_from_cursor(egui::text::CCursor::new(characters))
        .translate(output.galley_pos.to_vec2());
    ui.scroll_to_rect(rect, Some(egui::Align::Center));
}

/// The format and view mode, then each notice in order.
fn render_header(ui: &mut egui::Ui, format: BodyFormat, notices: &[Notice], key: &Key) {
    ui.weak(format!("Format: {} · {}", format.label(), key.mode.label()));
    for notice in notices {
        match notice.level {
            NoticeLevel::Info => {
                ui.weak(notice.text.as_str());
            }
            NoticeLevel::Warning => {
                ui.colored_label(Palette::of(ui).warning, notice.text.as_str());
            }
        }
    }
}

/// Draws `lines` with a line number gutter, laying out only the rows that
/// are scrolled into view. Long lines scroll sideways instead of wrapping so
/// each row is one source line.
fn render_numbered_lines(
    ui: &mut egui::Ui,
    lines: &[LayoutJob],
    side: BodySide,
    scroll_to: Option<usize>,
) {
    let font = egui::TextStyle::Monospace.resolve(ui.style());
    let row_height = ui.fonts_mut(|fonts| fonts.row_height(&font));
    let digits = lines.len().to_string().len();
    let mut area = egui::ScrollArea::both()
        .id_salt(("body-lines", side))
        .max_height(LINE_VIEW_HEIGHT)
        .auto_shrink([false, true]);
    if let Some(line) = scroll_to {
        // `show_rows` places rows `row_height` plus item spacing apart.
        let spacing = ui.spacing().item_spacing.y;
        #[expect(
            clippy::cast_precision_loss,
            reason = "line counts of an 8 KiB preview fit in f32"
        )]
        let top = line as f32 * (row_height + spacing);
        area = area.vertical_scroll_offset((top - LINE_VIEW_HEIGHT / 2.0).max(0.0));
    }
    area.show_rows(ui, row_height, lines.len(), |ui, rows| {
        for number in rows {
            let Some(line) = lines.get(number) else {
                break;
            };
            // `horizontal` would pad rows to the button height and break
            // the fixed row height `show_rows` relies on.
            let row_size = egui::vec2(ui.available_width(), row_height);
            let layout = egui::Layout::left_to_right(egui::Align::Min);
            ui.allocate_ui_with_layout(row_size, layout, |ui| {
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(format!("{:>digits$}", number + 1))
                            .font(font.clone())
                            .weak(),
                    )
                    .selectable(false),
                );
                ui.add(egui::Label::new(line.clone()).wrap_mode(egui::TextWrapMode::Extend));
            });
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(
        cache: &mut BodyViewCache,
        ctx: &egui::Context,
        source: &BodySource<'_>,
        mode: ViewMode,
    ) {
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| cache.show(ui, source, mode));
        output.textures_delta.clear();
    }

    fn partial_capture() -> Notice {
        Notice::warning(
            "Only part of the body was captured (at most 8 KiB is kept); the view ends where the capture ends.",
        )
    }

    /// Notices of the body last shown on the response side.
    fn notices(cache: &BodyViewCache) -> Vec<Notice> {
        match &cache.response.shown {
            Some((_, Content::Ready { notices, .. } | Content::Tree { notices, .. })) => {
                notices.clone()
            }
            _ => Vec::new(),
        }
    }

    /// Parses a test media type that lives for the whole test run.
    fn media(value: &str) -> Option<&'static MediaType> {
        MediaType::parse(value).map(|media_type| &*Box::leak(Box::new(media_type)))
    }

    fn source(record: u64, bytes: &[u8]) -> BodySource<'_> {
        BodySource {
            record,
            side: BodySide::Response,
            bytes,
            encodings: &[],
            media_type: media("application/json"),
            capture: Capture::Whole,
            search: None,
        }
    }

    #[test]
    fn css_preview_caches_gutter_and_warns_when_truncated() {
        let ctx = egui::Context::default();
        let mut cache = BodyViewCache::default();
        let css = BodySource {
            media_type: media("text/css"),
            capture: Capture::Truncated,
            ..source(1, b"a{color:red}")
        };
        run(&mut cache, &ctx, &css, ViewMode::Pretty);
        run(&mut cache, &ctx, &css, ViewMode::Pretty);
        assert_eq!(cache.builds(), 1);
        if let Some((
            _,
            Content::Ready {
                job,
                lines,
                notices,
                ..
            },
        )) = &cache.response.shown
        {
            assert_eq!(job.text, "a{\n  color:red\n}");
            let texts: Vec<_> = lines
                .as_deref()
                .unwrap_or_default()
                .iter()
                .map(|line| line.text.as_str())
                .collect();
            assert_eq!(texts, ["a{", "  color:red", "}"]);
            assert_eq!(notices, &[partial_capture()]);
        } else {
            assert_eq!(cache.builds(), 0, "expected prepared CSS content");
        }
        run(&mut cache, &ctx, &css, ViewMode::Raw);
        if let Some((_, Content::Ready { job, lines, .. })) = &cache.response.shown {
            assert_eq!(job.text.as_bytes(), css.bytes);
            let texts: Vec<_> = lines
                .as_deref()
                .unwrap_or_default()
                .iter()
                .map(|line| line.text.as_str())
                .collect();
            assert_eq!(texts, ["a{color:red}"]);
        }
    }

    #[test]
    fn complete_compressed_css_uses_decoder_notice_without_incomplete_warning()
    -> Result<(), Box<dyn std::error::Error>> {
        use std::io::Write;

        let text = format!("{}{}", "a{color:red}".repeat(600), " ".repeat(2000));
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(text.as_bytes())?;
        let compressed = encoder.finish()?;
        let css = BodySource {
            media_type: media("text/css"),
            encodings: &["gzip"],
            ..source(1, &compressed)
        };
        let ctx = egui::Context::default();
        let mut cache = BodyViewCache::default();
        run(&mut cache, &ctx, &css, ViewMode::Pretty);
        let Some((_, Content::Ready { notices, job, .. })) = &cache.response.shown else {
            return Err("expected prepared CSS content".into());
        };
        assert_eq!(
            notices,
            &[Notice::info(
                "Content-Encoding: gzip · first 8 KiB of decoded bytes"
            )]
        );
        assert!(job.text.contains("color:red"));
        Ok(())
    }

    #[test]
    fn rebuilds_only_when_the_body_or_view_changes() {
        let ctx = egui::Context::default();
        let mut cache = BodyViewCache::default();
        let first = 1;
        let body = br#"{"a":[1,2]}"#;
        run(&mut cache, &ctx, &source(first, body), ViewMode::Pretty);
        run(&mut cache, &ctx, &source(first, body), ViewMode::Pretty);
        assert_eq!(cache.builds(), 1);
        run(&mut cache, &ctx, &source(first, body), ViewMode::Raw);
        assert_eq!(cache.builds(), 2);
        let request = BodySource {
            side: BodySide::Request,
            ..source(first, body)
        };
        run(&mut cache, &ctx, &request, ViewMode::Raw);
        run(&mut cache, &ctx, &source(first, body), ViewMode::Raw);
        assert_eq!(cache.builds(), 3, "each side keeps its own entry");
        run(&mut cache, &ctx, &source(2, body), ViewMode::Raw);
        assert_eq!(cache.builds(), 4);
        // The theme buttons switch the theme in the middle of a frame.
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            ui.ctx().set_theme(Theme::Light);
            cache.show(ui, &source(2, body), ViewMode::Raw);
        });
        output.textures_delta.clear();
        assert_eq!(cache.builds(), 5);
        let text_color = match &cache.response.shown {
            Some((_, Content::Ready { job, .. })) => job.sections.first().map(|s| s.format.color),
            _ => None,
        };
        assert_eq!(
            text_color,
            Some(ctx.style_of(Theme::Light).visuals.text_color()),
            "colors follow the new theme in the frame it is switched"
        );
    }

    #[test]
    fn pretty_shows_indented_text_and_raw_the_original() {
        let ctx = egui::Context::default();
        let mut cache = BodyViewCache::default();
        let body = br#"{"a":true}"#;
        let shown = |cache: &BodyViewCache| match &cache.response.shown {
            Some((_, Content::Ready { job, .. })) => job.text.clone(),
            _ => String::new(),
        };
        run(&mut cache, &ctx, &source(1, body), ViewMode::Pretty);
        assert_eq!(shown(&cache), "{\n  \"a\": true\n}");
        run(&mut cache, &ctx, &source(1, body), ViewMode::Raw);
        assert_eq!(shown(&cache).as_bytes(), body);
    }

    fn tree_builds(cache: &BodyViewCache) -> u64 {
        cache.response.tree.builds
    }

    #[test]
    fn trees_keep_parse_and_open_rows_across_modes_and_themes() -> Result<(), String> {
        for (content_type, body) in [
            ("application/json", &br#"{"b":{"c":1},"a":2,"b":1e2}"#[..]),
            ("application/xml", b"<a><b><c>1</c></b><d/></a>"),
        ] {
            let ctx = egui::Context::default();
            ctx.set_theme(Theme::Dark);
            let mut cache = BodyViewCache::default();
            let body = BodySource {
                media_type: media(content_type),
                ..source(1, body)
            };
            run(&mut cache, &ctx, &body, ViewMode::Tree);
            assert!(matches!(
                &cache.response.shown,
                Some((_, Content::Tree { .. }))
            ));
            let open = |cache: &mut BodyViewCache| {
                cache
                    .response
                    .tree
                    .view()
                    .map_or_else(Vec::new, |view| view.open_rows())
            };
            let initial = open(&mut cache);
            cache
                .response
                .tree
                .view()
                .ok_or("missing tree")?
                .collapse_all();
            run(&mut cache, &ctx, &body, ViewMode::Raw);
            ctx.set_theme(Theme::Light);
            run(&mut cache, &ctx, &body, ViewMode::Tree);
            assert_eq!(tree_builds(&cache), 1, "{content_type} parsed once");
            assert_eq!(cache.builds(), 3, "the text view still follows the theme");
            assert!(open(&mut cache).iter().all(|open| !open));
            assert_ne!(
                open(&mut cache),
                initial,
                "{content_type} kept its rows closed"
            );
            let longer = BodySource {
                capture: Capture::Truncated,
                ..body
            };
            run(&mut cache, &ctx, &longer, ViewMode::Tree);
            run(&mut cache, &ctx, &source(2, body.bytes), ViewMode::Tree);
            assert_eq!(tree_builds(&cache), 3, "other bytes build a new tree");
        }
        let ctx = egui::Context::default();
        let mut cache = BodyViewCache::default();
        run(&mut cache, &ctx, &source(3, b"[1,"), ViewMode::Tree);
        assert!(matches!(
            &cache.response.shown,
            Some((_, Content::Ready { notices, .. }))
                if notices.iter().any(|notice| notice.text.starts_with("Showing Pretty"))
        ));
        Ok(())
    }

    #[test]
    fn tree_mode_builds_an_xml_tree_and_falls_back_for_other_formats() {
        let ctx = egui::Context::default();
        let mut cache = BodyViewCache::default();
        let xml = BodySource {
            media_type: media("application/rss+xml"),
            ..source(1, b"<rss><channel><title>A</title></channel></rss>")
        };
        run(&mut cache, &ctx, &xml, ViewMode::Tree);
        assert!(matches!(
            &cache.response.shown,
            Some((_, Content::Tree { .. }))
        ));
        let text = BodySource {
            media_type: media("text/plain"),
            ..source(2, b"plain text")
        };
        run(&mut cache, &ctx, &text, ViewMode::Tree);
        let fallback = match &cache.response.shown {
            Some((_, Content::Ready { notices, job, .. })) => {
                Some((notices.clone(), job.text.clone()))
            }
            _ => None,
        };
        assert_eq!(
            fallback,
            Some((
                vec![Notice::warning(
                    "Tree view is not available for Text; showing Pretty."
                )],
                "plain text".to_owned()
            ))
        );
    }

    #[test]
    fn numbers_script_lines_and_warns_about_partial_capture() {
        let ctx = egui::Context::default();
        let mut cache = BodyViewCache::default();
        let script = BodySource {
            media_type: media("application/javascript"),
            capture: Capture::Truncated,
            ..source(1, b"f(function(){a();b()}); x()")
        };
        run(&mut cache, &ctx, &script, ViewMode::Pretty);
        let (lines, notices) = match &cache.response.shown {
            Some((_, Content::Ready { lines, notices, .. })) => (lines.as_deref(), notices.clone()),
            _ => (None, Vec::new()),
        };
        let texts: Vec<_> = lines
            .unwrap_or_default()
            .iter()
            .map(|line| line.text.as_str())
            .collect();
        assert_eq!(texts, ["f(function(){", "  a();", "  b()", "});", "x()"]);
        assert_eq!(notices, [partial_capture()]);
        run(&mut cache, &ctx, &script, ViewMode::Raw);
        assert!(matches!(
            &cache.response.shown,
            Some((_, Content::Ready { lines: Some(lines), .. })) if lines.len() == 1
        ));
        run(
            &mut cache,
            &ctx,
            &source(2, br#"{"a":1}"#),
            ViewMode::Pretty,
        );
        assert!(matches!(
            &cache.response.shown,
            Some((_, Content::Ready { lines: None, .. }))
        ));
    }

    #[test]
    fn every_format_shows_the_same_partial_capture_notice() {
        for (content_type, body) in [
            ("application/json", &br#"{"a":[1,"#[..]),
            ("text/css", b"a{color:red}"),
            ("application/javascript", b"a();"),
            ("text/html", b"<p>a</p>"),
            ("application/xml", b"<a><b>1</b></a>"),
            ("application/x-www-form-urlencoded", b"a=1"),
            ("text/plain", b"plain"),
        ] {
            for mode in ViewMode::ALL {
                let ctx = egui::Context::default();
                let mut cache = BodyViewCache::default();
                let partial = BodySource {
                    media_type: media(content_type),
                    capture: Capture::Truncated,
                    ..source(1, body)
                };
                run(&mut cache, &ctx, &partial, mode);
                let shown = notices(&cache);
                assert_eq!(
                    shown
                        .iter()
                        .filter(|notice| **notice == partial_capture())
                        .count(),
                    1,
                    "{content_type} {mode:?}: {shown:?}"
                );
                let whole = BodySource {
                    capture: Capture::Whole,
                    ..partial
                };
                run(&mut cache, &ctx, &whole, mode);
                assert!(
                    !notices(&cache).contains(&partial_capture()),
                    "{content_type} {mode:?}"
                );
            }
        }
    }

    #[test]
    fn lists_notices_in_display_order() {
        let ctx = egui::Context::default();
        let mut cache = BodyViewCache::default();
        let script = BodySource {
            media_type: media("application/javascript"),
            capture: Capture::Truncated,
            ..source(1, b"f(\"unfinished")
        };
        run(&mut cache, &ctx, &script, ViewMode::Tree);
        let shown = notices(&cache);
        let levels: Vec<_> = shown.iter().map(|notice| notice.level).collect();
        assert_eq!(levels, [NoticeLevel::Warning; 3], "{shown:?}");
        assert_eq!(shown.first(), Some(&partial_capture()));
        assert_eq!(
            shown.get(1),
            Some(&Notice::warning(
                "Tree view is not available for JavaScript; showing Pretty."
            ))
        );
        assert!(
            shown
                .get(2)
                .is_some_and(|notice| notice.text.contains("unfinished string"))
        );

        let xml = BodySource {
            media_type: media("application/xml"),
            capture: Capture::Truncated,
            ..source(2, b"<a><b>1</b>")
        };
        run(&mut cache, &ctx, &xml, ViewMode::Tree);
        let shown = notices(&cache);
        assert!(matches!(
            &cache.response.shown,
            Some((_, Content::Tree { .. }))
        ));
        assert_eq!(shown.first(), Some(&partial_capture()));
        assert!(
            shown
                .get(1)
                .is_some_and(|notice| notice.text.starts_with("XML is incomplete")),
            "{shown:?}"
        );
    }

    #[test]
    fn reads_the_capture_reason_from_lifecycle_and_retention() {
        use BodyLifecycle::{Cancelled, Complete, Interrupted, Streaming};
        for (lifecycle, retention, expected) in [
            (Complete, BodyRetention::Prefix, Capture::Whole),
            (Streaming, BodyRetention::Prefix, Capture::Streaming),
            (Interrupted, BodyRetention::Prefix, Capture::Ended),
            (Cancelled, BodyRetention::Prefix, Capture::Ended),
            (Complete, BodyRetention::Truncated, Capture::Truncated),
            (Streaming, BodyRetention::Truncated, Capture::Truncated),
            (Interrupted, BodyRetention::Truncated, Capture::Truncated),
        ] {
            assert_eq!(
                Capture::of(lifecycle, retention),
                expected,
                "{lifecycle:?} {retention:?}"
            );
        }
    }

    #[test]
    fn names_why_the_view_ends_early() {
        let ctx = egui::Context::default();
        let mut cache = BodyViewCache::default();
        for (record, capture, expected) in [
            (1, Capture::Whole, None),
            (2, Capture::Truncated, Some(partial_capture())),
            (
                3,
                Capture::Streaming,
                Some(Notice::warning(
                    "The body is still being received; the view shows what has arrived so far.",
                )),
            ),
            (
                4,
                Capture::Ended,
                Some(Notice::warning(
                    "The body ended early (interrupted or cancelled); the view shows what was received.",
                )),
            ),
        ] {
            let body = BodySource {
                capture,
                ..source(record, br#"{"a":1}"#)
            };
            run(&mut cache, &ctx, &body, ViewMode::Pretty);
            assert_eq!(
                notices(&cache),
                expected.into_iter().collect::<Vec<_>>(),
                "{capture:?}"
            );
        }
    }

    fn glyph_x(galley: &Galley, text: &str, needle: &str) -> Result<f32, String> {
        let byte_index = text
            .find(needle)
            .ok_or_else(|| format!("needle {needle:?} not found in {text:?}"))?;
        let char_index = text
            .get(..byte_index)
            .ok_or_else(|| format!("invalid byte offset {byte_index}"))?
            .chars()
            .count();
        Ok(galley
            .pos_from_cursor(egui::text::CCursor::new(char_index))
            .min
            .x)
    }

    fn cached_text_galley(cache: &BodyViewCache) -> Result<(&LayoutJob, Arc<Galley>), String> {
        match &cache.response.shown {
            Some((
                _,
                Content::Ready {
                    job,
                    galley: Some((_, galley)),
                    ..
                },
            )) => Ok((job, Arc::clone(galley))),
            _ => Err("expected laid-out text body".to_owned()),
        }
    }

    fn assert_three_lines_aligned(
        galley: &Galley,
        text: &str,
        lines: [&str; 3],
        next_field: &str,
    ) -> Result<(), String> {
        let first_x = glyph_x(galley, text, lines[0])?;
        let second_x = glyph_x(galley, text, lines[1])?;
        let third_x = glyph_x(galley, text, lines[2])?;
        let next_x = glyph_x(galley, text, next_field)?;
        assert!((first_x - second_x).abs() < 0.5, "{first_x} vs {second_x}");
        assert!((first_x - third_x).abs() < 0.5, "{first_x} vs {third_x}");
        assert!(first_x > next_x, "value {first_x} <= next field {next_x}");
        Ok(())
    }

    #[test]
    fn aligns_nested_xml_html_and_json_multiline_values() -> Result<(), String> {
        let ctx = egui::Context::default();
        let mut cache = BodyViewCache::default();

        let xml_bytes =
            b"<root><item title=\"attr1\nattr2\"><note>line1\nline2\nline3</note><next>ok</next></item></root>";
        let xml = BodySource {
            media_type: media("application/xml"),
            ..source(10, xml_bytes)
        };
        run(&mut cache, &ctx, &xml, ViewMode::Pretty);
        let (job, galley) = cached_text_galley(&cache)?;
        assert!(job.text.contains("line1\nline2\nline3"), "{}", job.text);
        assert!(job.text.contains("\"attr1\nattr2\""), "{}", job.text);
        assert_three_lines_aligned(&galley, &job.text, ["line1", "line2", "line3"], "<next>")?;
        let attr_first_x = glyph_x(&galley, &job.text, "attr1")?;
        let attr_second_x = glyph_x(&galley, &job.text, "attr2")?;
        assert!((attr_first_x - attr_second_x).abs() < 0.5);

        let html_bytes =
            b"<section><textarea name=\"bio\">alpha\nbeta\ngamma</textarea><p>after</p></section>";
        let html = BodySource {
            media_type: media("text/html"),
            ..source(11, html_bytes)
        };
        run(&mut cache, &ctx, &html, ViewMode::Pretty);
        let (job, galley) = cached_text_galley(&cache)?;
        assert!(job.text.contains("alpha\nbeta\ngamma"), "{}", job.text);
        assert_three_lines_aligned(&galley, &job.text, ["alpha", "beta", "gamma"], "<p>")?;

        let json_bytes = b"{\"outer\":{\"note\":\"one\ntwo\nthree\",\"next\":1}}";
        let json = BodySource {
            media_type: media("application/json"),
            ..source(12, json_bytes)
        };
        run(&mut cache, &ctx, &json, ViewMode::Pretty);
        let (job, galley) = cached_text_galley(&cache)?;
        assert!(job.text.contains("\"one\ntwo\nthree\""), "{}", job.text);
        assert_three_lines_aligned(&galley, &job.text, ["one", "two", "three"], "\"next\"")?;
        Ok(())
    }

    #[test]
    fn preserves_raw_and_tree_copy_values() -> Result<(), String> {
        let ctx = egui::Context::default();
        let mut cache = BodyViewCache::default();

        let xml_bytes = b"<root><note>line1\nline2\nline3</note></root>";
        let xml = BodySource {
            media_type: media("application/xml"),
            ..source(13, xml_bytes)
        };
        run(&mut cache, &ctx, &xml, ViewMode::Raw);
        let (raw_job, _) = cached_text_galley(&cache)?;
        assert_eq!(raw_job.text.as_bytes(), xml_bytes);
        assert!(
            raw_job
                .sections
                .iter()
                .all(|section| section.leading_space == 0.0)
        );

        let xml_tree = tree::build(
            BodyFormat::Xml,
            std::str::from_utf8(xml_bytes).map_err(|e| e.to_string())?,
        )
        .ok_or("missing XML tree")?
        .map_err(|e| e.to_string())?;
        assert!(
            xml_tree
                .nodes()
                .iter()
                .any(|node| node.value == "<note>line1\nline2\nline3</note>")
        );

        let valid_json = r#"{"note":"line1\n  line2\nline3"}"#;
        let json_tree = tree::build(BodyFormat::Json, valid_json)
            .ok_or("missing JSON tree")?
            .map_err(|e| e.to_string())?;
        assert!(
            json_tree
                .nodes()
                .iter()
                .any(|node| node.value == r#""line1\n  line2\nline3""#)
        );
        Ok(())
    }

    #[test]
    fn aligns_js_line_numbered_multiline_values_and_keeps_raw_unpadded() -> Result<(), String> {
        let ctx = egui::Context::default();
        let mut cache = BodyViewCache::default();

        let js_bytes = b"const cfg={a:{msg:`row1\nrow2\nrow3`,next:1}};";
        let js = BodySource {
            media_type: media("application/javascript"),
            ..source(14, js_bytes)
        };
        run(&mut cache, &ctx, &js, ViewMode::Pretty);
        let js_lines = match &cache.response.shown {
            Some((
                _,
                Content::Ready {
                    lines: Some(lines), ..
                },
            )) => lines.clone(),
            _ => return Err("expected line-numbered JS body".to_owned()),
        };
        let row1_idx = js_lines
            .iter()
            .position(|line| line.text.contains("row1"))
            .ok_or("missing row1")?;
        let first_line = js_lines.get(row1_idx).ok_or("missing row1 line")?;
        let second_line = js_lines.get(row1_idx + 1).ok_or("missing row2 line")?;
        let third_line = js_lines.get(row1_idx + 2).ok_or("missing row3 line")?;
        assert_eq!(second_line.text, "row2");
        assert_eq!(third_line.text, "row3`,");
        let mut check: Result<(), String> = Ok(());
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            check = (|| {
                let g1 = ui.fonts_mut(|f| f.layout_job(first_line.clone()));
                let g2 = ui.fonts_mut(|f| f.layout_job(second_line.clone()));
                let g3 = ui.fonts_mut(|f| f.layout_job(third_line.clone()));
                let first_x = glyph_x(&g1, &first_line.text, "row1")?;
                let second_x = glyph_x(&g2, &second_line.text, "row2")?;
                let third_x = glyph_x(&g3, &third_line.text, "row3")?;
                assert!((first_x - second_x).abs() < 0.5, "{first_x} vs {second_x}");
                assert!((first_x - third_x).abs() < 0.5, "{first_x} vs {third_x}");
                Ok(())
            })();
        });
        output.textures_delta.clear();
        check?;

        run(&mut cache, &ctx, &js, ViewMode::Raw);
        let Some((_, Content::Ready { job, lines, .. })) = &cache.response.shown else {
            return Err("expected raw JS body".to_owned());
        };
        assert_eq!(job.text.as_bytes(), js_bytes);
        assert!(
            lines
                .as_deref()
                .unwrap_or_default()
                .iter()
                .flat_map(|line| &line.sections)
                .all(|section| section.leading_space == 0.0)
        );
        Ok(())
    }

    #[test]
    fn preserves_multiline_alignment_with_active_search_highlights() -> Result<(), String> {
        let ctx = egui::Context::default();
        let mut cache = BodyViewCache::default();
        let query = Query {
            generation: 1,
            matcher: http_proxy::analysis::search::Matcher::new(
                "line",
                http_proxy::analysis::search::MatchOptions::default(),
            )
            .map_err(|e| e.to_string())?,
            scope: http_proxy::analysis::search::SearchScope::All,
        };
        let xml = BodySource {
            media_type: media("application/xml"),
            search: Some(&query),
            ..source(
                20,
                b"<root><note>line1\nline2\nline3</note><other>line4</other></root>",
            )
        };
        run(&mut cache, &ctx, &xml, ViewMode::Pretty);
        let (job, galley) = cached_text_galley(&cache)?;
        assert!(job.text.contains("line1\nline2\nline3"));
        let x1 = glyph_x(&galley, &job.text, "line1")?;
        let x2 = glyph_x(&galley, &job.text, "line2")?;
        let x3 = glyph_x(&galley, &job.text, "line3")?;
        assert!((x1 - x2).abs() < 0.5, "x1={x1}, x2={x2}");
        assert!((x1 - x3).abs() < 0.5, "x1={x1}, x3={x3}");
        Ok(())
    }

    #[test]
    fn clamps_multiline_continuation_indent_in_narrow_viewport() -> Result<(), String> {
        let ctx = egui::Context::default();
        let mut cache = BodyViewCache::default();
        let xml = BodySource {
            media_type: media("application/xml"),
            ..source(
                21,
                b"<root><very_long_nested_field_name_for_narrow_window>first\nsecond\nthird</very_long_nested_field_name_for_narrow_window></root>",
            )
        };
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(640.0, 420.0),
            )),
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |ui| {
            ui.set_max_width(240.0);
            cache.show(ui, &xml, ViewMode::Pretty);
        });
        output.textures_delta.clear();

        let (job, galley) = cached_text_galley(&cache)?;
        let x_second = glyph_x(&galley, &job.text, "second")?;
        let x_third = glyph_x(&galley, &job.text, "third")?;
        assert!(
            x_second > 0.0,
            "continuation should remain visibly indented"
        );
        assert!(
            x_second <= 120.5,
            "indent {x_second} should be clamped to at most half of narrow width"
        );
        assert!(
            (x_second - x_third).abs() < 0.5,
            "clamped continuations must still align: x_second={x_second}, x_third={x_third}"
        );
        Ok(())
    }

    #[test]
    fn trailing_newline_does_not_enable_break_anywhere_on_wide_viewport() -> Result<(), String> {
        let ctx = egui::Context::default();
        let mut cache = BodyViewCache::default();
        let truncated_xml = BodySource {
            media_type: media("application/xml"),
            capture: Capture::Truncated,
            ..source(
                22,
                b"<root><note>first\nsecond\nalpha beta gamma delta epsilon zeta\n",
            )
        };
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1000.0, 600.0),
            )),
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |ui| {
            ui.set_max_width(300.0);
            cache.show(ui, &truncated_xml, ViewMode::Pretty);
        });
        output.textures_delta.clear();

        let (job, galley) = cached_text_galley(&cache)?;
        assert!(
            !galley.job.wrap.break_anywhere,
            "normal trailing newline row must not trigger break_anywhere"
        );
        let first_x = glyph_x(&galley, &job.text, "first")?;
        let second_x = glyph_x(&galley, &job.text, "second")?;
        assert!((first_x - second_x).abs() < 0.5, "{first_x} vs {second_x}");
        let row_texts: Vec<String> = galley.rows.iter().map(|row| row.text()).collect();
        assert!(
            row_texts.iter().any(|row| row.contains("epsilon")),
            "words should stay intact on word-boundary wraps: {row_texts:?}"
        );
        Ok(())
    }

    #[test]
    fn aligns_closing_tag_when_source_tail_indent_is_shorter_than_depth() -> Result<(), String> {
        let ctx = egui::Context::default();
        let mut cache = BodyViewCache::default();

        let xml = BodySource {
            media_type: media("application/xml"),
            ..source(
                23,
                b"<root><outer><note>first\nsecond\nthird\n  </note><next>ok</next></outer></root>",
            )
        };
        run(&mut cache, &ctx, &xml, ViewMode::Pretty);
        let (job, galley) = cached_text_galley(&cache)?;
        assert!(job.text.contains("\n  </note>"), "{}", job.text);
        let open_x = glyph_x(&galley, &job.text, "<note>")?;
        let close_x = glyph_x(&galley, &job.text, "</note>")?;
        let next_x = glyph_x(&galley, &job.text, "<next>")?;
        assert!((open_x - close_x).abs() < 0.5, "{open_x} vs {close_x}");
        assert!((open_x - next_x).abs() < 0.5, "{open_x} vs {next_x}");

        let html = BodySource {
            media_type: media("text/html"),
            ..source(
                24,
                b"<div><section><textarea>\nline1\nline2\n  </textarea><p>next</p></section></div>",
            )
        };
        run(&mut cache, &ctx, &html, ViewMode::Pretty);
        let (job, galley) = cached_text_galley(&cache)?;
        assert!(job.text.contains("\n  </textarea>"), "{}", job.text);
        let html_open_x = glyph_x(&galley, &job.text, "<textarea>")?;
        let html_close_x = glyph_x(&galley, &job.text, "</textarea>")?;
        let html_next_x = glyph_x(&galley, &job.text, "<p>")?;
        assert!(
            (html_open_x - html_close_x).abs() < 0.5,
            "{html_open_x} vs {html_close_x}"
        );
        assert!(
            (html_open_x - html_next_x).abs() < 0.5,
            "{html_open_x} vs {html_next_x}"
        );
        Ok(())
    }

    fn find_text_shape(
        shapes: &[egui::epaint::ClippedShape],
        predicate: &impl Fn(&str) -> bool,
    ) -> Option<(egui::Pos2, Arc<Galley>)> {
        fn walk(
            shape: &egui::Shape,
            predicate: &impl Fn(&str) -> bool,
        ) -> Option<(egui::Pos2, Arc<Galley>)> {
            match shape {
                egui::Shape::Text(text) if predicate(text.galley.text()) => {
                    Some((text.pos, Arc::clone(&text.galley)))
                }
                egui::Shape::Vec(items) => items.iter().find_map(|item| walk(item, predicate)),
                _ => None,
            }
        }
        shapes
            .iter()
            .find_map(|clipped| walk(&clipped.shape, predicate))
    }

    fn cursor_screen_pos(
        origin: egui::Pos2,
        galley: &Galley,
        byte_index: usize,
    ) -> Result<egui::Pos2, String> {
        let char_index = galley
            .text()
            .get(..byte_index)
            .ok_or_else(|| format!("invalid byte index {byte_index}"))?
            .chars()
            .count();
        let rect = galley.pos_from_cursor(egui::text::CCursor::new(char_index));
        Ok(egui::pos2(
            origin.x + rect.min.x,
            origin.y + rect.center().y,
        ))
    }

    fn run_frame(
        ctx: &egui::Context,
        time: f64,
        events: Vec<egui::Event>,
        body: impl FnMut(&mut egui::Ui),
    ) -> egui::FullOutput {
        let input = egui::RawInput {
            time: Some(time),
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(800.0, 900.0),
            )),
            events,
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, body);
        output.textures_delta.clear();
        output
    }

    fn copied_texts(output: &egui::FullOutput) -> Vec<String> {
        output
            .platform_output
            .commands
            .iter()
            .filter_map(|cmd| match cmd {
                egui::OutputCommand::CopyText(text) => Some(text.clone()),
                _ => None,
            })
            .collect()
    }

    fn drag_select(
        ctx: &egui::Context,
        time: &mut f64,
        from: egui::Pos2,
        to: egui::Pos2,
        mut body: impl FnMut(&mut egui::Ui),
    ) {
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
            let _ = run_frame(ctx, *time, events, &mut body);
        }
    }

    fn click_at(
        ctx: &egui::Context,
        time: &mut f64,
        pos: egui::Pos2,
        mut body: impl FnMut(&mut egui::Ui),
    ) {
        *time += 0.05;
        let _ = run_frame(
            ctx,
            *time,
            vec![
                egui::Event::PointerMoved(pos),
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
            &mut body,
        );
        *time += 0.02;
        let _ = run_frame(
            ctx,
            *time,
            vec![egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }],
            &mut body,
        );
    }

    fn key_press(key: egui::Key, modifiers: egui::Modifiers) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        }
    }

    fn drag_copy_slice(
        ctx: &egui::Context,
        time: &mut f64,
        pos: egui::Pos2,
        galley: &Galley,
        slice: &str,
        mut draw: impl FnMut(&mut egui::Ui),
    ) -> Result<Vec<String>, String> {
        let start = galley
            .text()
            .find(slice)
            .ok_or_else(|| format!("missing slice {slice:?}"))?;
        let end = start + slice.len();
        drag_select(
            ctx,
            time,
            cursor_screen_pos(pos, galley, start)?,
            cursor_screen_pos(pos, galley, end)?,
            &mut draw,
        );
        *time += 0.05;
        let out = run_frame(ctx, *time, vec![egui::Event::Copy], &mut draw);
        Ok(copied_texts(&out))
    }

    #[test]
    fn copies_pretty_request_and_response_with_drag_select_all_and_clicks() -> Result<(), String> {
        let ctx = egui::Context::default();
        let mut cache = BodyViewCache::default();
        let mut time = 0.0;
        let req_source = BodySource {
            side: BodySide::Request,
            ..source(30, br#"{"req":  "alpha  beta","id":1}"#)
        };
        let resp_source = BodySource {
            side: BodySide::Response,
            ..source(30, br#"{"resp":  "gamma  delta","ok":true}"#)
        };
        let mut draw_pretty = |ui: &mut egui::Ui| {
            cache.show(ui, &req_source, ViewMode::Pretty);
            ui.separator();
            cache.show(ui, &resp_source, ViewMode::Pretty);
        };

        time += 0.05;
        let initial = run_frame(&ctx, time, Vec::new(), &mut draw_pretty);
        let expected_req = "{\n  \"req\": \"alpha  beta\",\n  \"id\": 1\n}";
        let expected_resp = "{\n  \"resp\": \"gamma  delta\",\n  \"ok\": true\n}";
        let (req_pos, req_galley) = find_text_shape(&initial.shapes, &|t| t == expected_req)
            .ok_or("missing Pretty request body galley")?;
        let (resp_pos, resp_galley) = find_text_shape(&initial.shapes, &|t| t == expected_resp)
            .ok_or("missing Pretty response body galley")?;

        let req_slice = "  \"req\": \"alpha  beta\",\n  \"id\": 1";
        let got_req = drag_copy_slice(
            &ctx,
            &mut time,
            req_pos,
            &req_galley,
            req_slice,
            &mut draw_pretty,
        )?;
        assert_eq!(got_req, vec![req_slice.to_owned()]);

        let resp_slice = "  \"resp\": \"gamma  delta\",\n  \"ok\": true";
        let got_resp = drag_copy_slice(
            &ctx,
            &mut time,
            resp_pos,
            &resp_galley,
            resp_slice,
            &mut draw_pretty,
        )?;
        assert_eq!(got_resp, vec![resp_slice.to_owned()]);

        // Ctrl+A in request body selects only the request body, not the response body.
        let req_click = expected_req.find("alpha").ok_or("missing alpha")?;
        click_at(
            &ctx,
            &mut time,
            cursor_screen_pos(req_pos, &req_galley, req_click)?,
            &mut draw_pretty,
        );
        time += 0.05;
        let select_all = run_frame(
            &ctx,
            time,
            vec![
                key_press(egui::Key::A, egui::Modifiers::COMMAND),
                egui::Event::Copy,
            ],
            &mut draw_pretty,
        );
        assert_eq!(copied_texts(&select_all), vec![expected_req.to_owned()]);

        // Double-click selects a word ("gamma") and triple-click selects the line in response body.
        time += 1.0;
        let gamma_byte = expected_resp.find("gamma").ok_or("missing gamma")?;
        let gamma_pos = cursor_screen_pos(resp_pos, &resp_galley, gamma_byte + 2)?;
        click_at(&ctx, &mut time, gamma_pos, &mut draw_pretty);
        click_at(&ctx, &mut time, gamma_pos, &mut draw_pretty);
        time += 0.05;
        let dbl_copy = run_frame(&ctx, time, vec![egui::Event::Copy], &mut draw_pretty);
        assert_eq!(copied_texts(&dbl_copy), vec!["gamma".to_owned()]);

        click_at(&ctx, &mut time, gamma_pos, &mut draw_pretty);
        time += 0.05;
        let triple_copy = run_frame(&ctx, time, vec![egui::Event::Copy], &mut draw_pretty);
        assert_eq!(
            copied_texts(&triple_copy),
            vec!["  \"resp\": \"gamma  delta\",".to_owned()]
        );
        Ok(())
    }

    #[test]
    fn copies_raw_request_and_response_preserving_original_spacing() -> Result<(), String> {
        let ctx = egui::Context::default();
        let mut cache = BodyViewCache::default();
        let mut time = 0.0;

        let req_raw = br#"{"req":  "alpha  beta","id":1}"#;
        let resp_raw = br#"{"resp":  "gamma  delta","ok":true}"#;
        let req_source = BodySource {
            side: BodySide::Request,
            ..source(36, req_raw)
        };
        let resp_source = BodySource {
            side: BodySide::Response,
            ..source(36, resp_raw)
        };
        let mut draw_raw = |ui: &mut egui::Ui| {
            cache.show(ui, &req_source, ViewMode::Raw);
            ui.separator();
            cache.show(ui, &resp_source, ViewMode::Raw);
        };

        time += 0.05;
        let initial = run_frame(&ctx, time, Vec::new(), &mut draw_raw);
        let expected_req = std::str::from_utf8(req_raw).map_err(|e| e.to_string())?;
        let expected_resp = std::str::from_utf8(resp_raw).map_err(|e| e.to_string())?;
        let (req_pos, req_galley) = find_text_shape(&initial.shapes, &|t| t == expected_req)
            .ok_or("missing Raw request body galley")?;
        let (resp_pos, resp_galley) = find_text_shape(&initial.shapes, &|t| t == expected_resp)
            .ok_or("missing Raw response body galley")?;

        let raw_req_slice = "\"req\":  \"alpha  beta\"";
        assert_eq!(
            drag_copy_slice(
                &ctx,
                &mut time,
                req_pos,
                &req_galley,
                raw_req_slice,
                &mut draw_raw
            )?,
            vec![raw_req_slice.to_owned()]
        );

        let raw_resp_slice = "\"resp\":  \"gamma  delta\"";
        assert_eq!(
            drag_copy_slice(
                &ctx,
                &mut time,
                resp_pos,
                &resp_galley,
                raw_resp_slice,
                &mut draw_raw
            )?,
            vec![raw_resp_slice.to_owned()]
        );
        Ok(())
    }

    #[test]
    fn read_only_body_rejects_text_edits_backspace_paste_and_cut() -> Result<(), String> {
        let ctx = egui::Context::default();
        let mut cache = BodyViewCache::default();
        let mut time = 0.0;
        let json = source(31, br#"{"name":"Alice","role":"admin"}"#);
        let expected = "{\n  \"name\": \"Alice\",\n  \"role\": \"admin\"\n}";

        time += 0.05;
        let initial = run_frame(&ctx, time, Vec::new(), |ui| {
            cache.show(ui, &json, ViewMode::Pretty);
        });
        let (pos, galley) =
            find_text_shape(&initial.shapes, &|t| t == expected).ok_or("missing JSON galley")?;

        let start = expected.find("Alice").ok_or("missing Alice")?;
        drag_select(
            &ctx,
            &mut time,
            cursor_screen_pos(pos, &galley, start)?,
            cursor_screen_pos(pos, &galley, start + "Alice".len())?,
            |ui| cache.show(ui, &json, ViewMode::Pretty),
        );

        // Attempt every mutating input event while focused with an active selection.
        time += 0.05;
        let _ = run_frame(
            &ctx,
            time,
            vec![
                egui::Event::Text("Bob".to_owned()),
                key_press(egui::Key::Backspace, egui::Modifiers::NONE),
                key_press(egui::Key::Delete, egui::Modifiers::NONE),
                key_press(egui::Key::Enter, egui::Modifiers::NONE),
                egui::Event::Paste("INJECTED".to_owned()),
                egui::Event::Cut,
            ],
            |ui| cache.show(ui, &json, ViewMode::Pretty),
        );

        let (job, _) = cached_text_galley(&cache)?;
        assert_eq!(job.text, expected, "body text must remain read-only");

        time += 0.05;
        let verify = run_frame(
            &ctx,
            time,
            vec![
                key_press(egui::Key::A, egui::Modifiers::COMMAND),
                egui::Event::Copy,
            ],
            |ui| cache.show(ui, &json, ViewMode::Pretty),
        );
        assert_eq!(copied_texts(&verify), vec![expected.to_owned()]);
        Ok(())
    }

    #[test]
    fn copies_multiline_values_without_visual_leading_space() -> Result<(), String> {
        let ctx = egui::Context::default();
        let mut cache = BodyViewCache::default();
        let mut time = 0.0;
        let xml = BodySource {
            media_type: media("application/xml"),
            ..source(
                32,
                b"<root><item><note>line1\nline2\n  line3</note><next>ok</next></item></root>",
            )
        };

        time += 0.05;
        let initial = run_frame(&ctx, time, Vec::new(), |ui| {
            cache.show(ui, &xml, ViewMode::Pretty);
        });
        let (job, _) = cached_text_galley(&cache)?;
        let expected_text = job.text.clone();
        assert!(
            job.sections
                .iter()
                .any(|section| section.leading_space > 0.0),
            "expected visual leading_space on multiline continuation lines"
        );

        let (pos, galley) = find_text_shape(&initial.shapes, &|t| t == expected_text)
            .ok_or("missing XML galley")?;
        let needle = "line1\nline2\n  line3";
        let copied = drag_copy_slice(&ctx, &mut time, pos, &galley, needle, |ui| {
            cache.show(ui, &xml, ViewMode::Pretty);
        })?;
        assert_eq!(
            copied,
            vec![needle.to_owned()],
            "visual leading_space must not leak into copied text"
        );
        Ok(())
    }

    #[test]
    fn resets_selection_on_record_and_mode_switch_without_panic() -> Result<(), String> {
        let ctx = egui::Context::default();
        let mut cache = BodyViewCache::default();
        let mut time = 0.0;
        let long_body = source(
            33,
            br#"{"first":"value-one","second":"value-two","third":"value-three"}"#,
        );
        let short_body = source(34, br#"{"x":1}"#);

        time += 0.05;
        let initial = run_frame(&ctx, time, Vec::new(), |ui| {
            cache.show(ui, &long_body, ViewMode::Pretty);
        });
        let (job, _) = cached_text_galley(&cache)?;
        let full_pretty = job.text.clone();
        let (pos, galley) = find_text_shape(&initial.shapes, &|t| t == full_pretty)
            .ok_or("missing long Pretty galley")?;
        let tail_start = full_pretty.find("\"third\"").ok_or("missing third field")?;
        drag_select(
            &ctx,
            &mut time,
            cursor_screen_pos(pos, &galley, tail_start)?,
            cursor_screen_pos(pos, &galley, full_pretty.len())?,
            |ui| cache.show(ui, &long_body, ViewMode::Pretty),
        );
        assert!(ctx.memory(egui::Memory::focused).is_some());

        // Switch Pretty -> Raw: selection and focus reset cleanly.
        time += 0.05;
        let switched_mode = run_frame(&ctx, time, vec![egui::Event::Copy], |ui| {
            cache.show(ui, &long_body, ViewMode::Raw);
        });
        assert_eq!(copied_texts(&switched_mode), Vec::<String>::new());
        assert!(ctx.memory(egui::Memory::focused).is_none());

        // Select near the end of long Raw body, then switch to a much shorter record.
        let (raw_job, _) = cached_text_galley(&cache)?;
        let full_raw = raw_job.text.clone();
        let (raw_pos, raw_galley) = find_text_shape(&switched_mode.shapes, &|t| t == full_raw)
            .ok_or("missing long Raw galley")?;
        drag_select(
            &ctx,
            &mut time,
            cursor_screen_pos(raw_pos, &raw_galley, full_raw.len() - 15)?,
            cursor_screen_pos(raw_pos, &raw_galley, full_raw.len())?,
            |ui| cache.show(ui, &long_body, ViewMode::Raw),
        );

        time += 0.05;
        let switched_record = run_frame(&ctx, time, vec![egui::Event::Copy], |ui| {
            cache.show(ui, &short_body, ViewMode::Raw);
        });
        assert_eq!(copied_texts(&switched_record), Vec::<String>::new());
        assert!(ctx.memory(egui::Memory::focused).is_none());
        Ok(())
    }

    #[test]
    fn numbered_lines_view_copies_code_without_line_numbers() -> Result<(), String> {
        let ctx = egui::Context::default();
        let mut cache = BodyViewCache::default();
        let mut time = 0.0;
        let css = BodySource {
            media_type: media("text/css"),
            ..source(35, b"a{color:red;margin:0}")
        };

        let mut draw = |ui: &mut egui::Ui| {
            cache.show(ui, &css, ViewMode::Pretty);
        };
        time += 0.05;
        let initial = run_frame(&ctx, time, Vec::new(), &mut draw);
        let (line1_pos, line1_galley) =
            find_text_shape(&initial.shapes, &|t| t == "a{").ok_or("missing first CSS line")?;
        let (line2_pos, line2_galley) = find_text_shape(&initial.shapes, &|t| t == "  color:red;")
            .ok_or("missing second CSS line")?;

        drag_select(
            &ctx,
            &mut time,
            cursor_screen_pos(line1_pos, &line1_galley, 0)?,
            cursor_screen_pos(line2_pos, &line2_galley, "  color:red;".len())?,
            &mut draw,
        );
        time += 0.05;
        let copied = run_frame(&ctx, time, vec![egui::Event::Copy], &mut draw);
        assert_eq!(copied_texts(&copied), vec!["a{\n  color:red;".to_owned()]);
        Ok(())
    }
}
