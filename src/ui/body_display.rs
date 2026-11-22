//! Decoded, formatted, and highlighted body text for the detail panel,
//! rebuilt only when the shown body or the way it is shown changes.

use crate::ui::body_format::{self, BodyFormat, Notice, NoticeLevel, ViewMode, highlight, tree};
use crate::ui::body_matches::{BodyMatches, Hit};
use crate::ui::body_view::{self, BodyText};
use crate::ui::search::Query;
use crate::ui::search::highlight::MatchColors;
use crate::ui::theme::{Palette, Theme};
use crate::ui::tree_view::TreeView;
use eframe::egui::{self, Galley, text::LayoutJob};
use http_proxy::{BodyLifecycle, BodyRetention, MediaType};
use std::sync::Arc;

/// Height of the numbered line view before it scrolls.
const LINE_VIEW_HEIGHT: f32 = 480.0;

/// Which body of an exchange is shown; both can be on screen at once.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(in crate::ui) enum BodySide {
    Request,
    Response,
}

/// A captured body preview to show.
pub(in crate::ui) struct BodySource<'a> {
    /// [`http_proxy::RecordId::value`] of the exchange the body belongs to.
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
                http_proxy::BODY_PREVIEW_BYTES / 1024
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
    tree: Result<TreeView, String>,
}

impl TreeCache {
    /// The tree of `text`, read as `format`; `None` when the format has
    /// none, and the reason when `text` could not be read as one.
    fn get(
        &mut self,
        source: &BodySource<'_>,
        format: BodyFormat,
        text: &str,
    ) -> Option<Result<(), String>> {
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
        Some(entry.tree.as_ref().map(|_| ()).map_err(Clone::clone))
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
            self.builds += 1;
            // Use the keyed theme's style: after a theme switch this frame's
            // style still belongs to the previous theme.
            let text_color = ui.ctx().style_of(key.theme).visuals.text_color();
            let content = build(source, &key, text_color, &mut side.tree);
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
    let format = BodyFormat::from_media_type(source.media_type);
    let mut notices = capture_notices(decoding, source.capture);
    if key.mode == ViewMode::Tree {
        match trees.get(source, format, &text) {
            Some(Ok(())) => {
                let incomplete = trees.view().and_then(|view| view.incomplete());
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
    let job = highlight::layout_job(&formatted.styled, &palette, text_color, &key.font);
    let lines = format
        .spec()
        .numbered
        .then(|| highlight::line_jobs(&formatted.styled, &palette, text_color, &key.font));
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
                let laid_out = ui.ctx().fonts_mut(|fonts| fonts.layout_job(wrapped));
                *galley = Some((width, Arc::clone(&laid_out)));
                laid_out
            }
        }
    };
    let output = egui::TextEdit::multiline(&mut text)
        .font(egui::TextStyle::Monospace)
        .desired_rows(8)
        .interactive(false)
        .layouter(&mut layouter)
        .show(ui);
    if let Some(hit) = scroll {
        scroll_to_hit(ui, &output, &job.text, &hit);
    }
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
}
