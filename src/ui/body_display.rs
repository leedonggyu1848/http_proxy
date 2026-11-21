//! Decoded, formatted, and highlighted body text for the detail panel,
//! rebuilt only when the shown body or the way it is shown changes.

use crate::ui::body_format::json_tree::Tree;
use crate::ui::body_format::xml::tree::XmlTree;
use crate::ui::body_format::{self, BodyFormat, ViewMode, highlight};
use crate::ui::body_view::{self, BodyText};
use crate::ui::theme::{Palette, Theme};
use crate::ui::xml_tree_view::XmlTreeView;
use eframe::egui::{self, Galley, text::LayoutJob};
use std::sync::Arc;
use std::{cell::RefCell, rc::Rc};

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
    pub(in crate::ui) content_type: Option<&'a str>,
    /// Whether the retained bytes are the whole body.
    pub(in crate::ui) complete: bool,
}

/// Everything the displayed text depends on. Retained previews only grow,
/// so their length identifies the bytes of one record's body.
#[derive(Clone, Debug, PartialEq)]
struct Key {
    record: u64,
    side: BodySide,
    retained: usize,
    complete: bool,
    mode: ViewMode,
    theme: Theme,
    font: egui::FontId,
}

enum Content {
    Unavailable(String),
    Tree {
        notice: Option<String>,
        view: TreeView,
    },
    Ready {
        format: BodyFormat,
        notice: Option<String>,
        fallback: Option<String>,
        warning: Option<String>,
        note: Option<String>,
        job: LayoutJob,
        /// One layout per line for formats shown with line numbers.
        lines: Option<Vec<LayoutJob>>,
        /// The last laid-out galley and the wrap width it was laid out for.
        galley: Option<(u32, Arc<Galley>)>,
    },
}

enum TreeView {
    Xml(XmlTreeView),
    Json(Rc<RefCell<Tree>>),
}

impl TreeView {
    fn show(&mut self, ui: &mut egui::Ui, key: &Key) {
        match self {
            Self::Json(tree) => {
                if !key.complete {
                    ui.weak("Incomplete body capture; tree contains only retained data.");
                }
                ui.push_id(("json-tree", key.record, key.side), |ui| {
                    tree.borrow_mut().show(ui);
                });
            }
            Self::Xml(view) => {
                ui.weak(format!(
                    "Format: {} · {}",
                    BodyFormat::Xml.label(),
                    key.mode.label()
                ));
                if let Some(incomplete) = view.incomplete() {
                    ui.colored_label(Palette::of(ui).warning, incomplete);
                }
                view.show(ui, ("xml-tree", key.record, key.side));
            }
        }
    }
}

#[derive(Default)]
struct JsonCache {
    entry: Option<JsonEntry>,
}

struct JsonEntry {
    key: (u64, usize, bool),
    tree: Result<Rc<RefCell<Tree>>, String>,
}

impl JsonCache {
    fn get(&mut self, source: &BodySource<'_>, text: &str) -> Result<Rc<RefCell<Tree>>, String> {
        let key = (source.record, source.bytes.len(), source.complete);
        if self.entry.as_ref().is_none_or(|entry| entry.key != key) {
            self.entry = Some(JsonEntry {
                key,
                tree: Tree::parse(text).map(|tree| Rc::new(RefCell::new(tree))),
            });
        }
        self.entry
            .as_ref()
            .ok_or("missing JSON cache entry")?
            .tree
            .clone()
    }
}

/// Highlighted text of the request and response bodies last shown.
#[derive(Default)]
pub(in crate::ui) struct BodyViewCache {
    request: Option<(Key, Content)>,
    response: Option<(Key, Content)>,
    builds: u64,
    request_json: JsonCache,
    response_json: JsonCache,
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
            complete: source.complete,
            mode,
            theme: ui.ctx().theme(),
            font: egui::TextStyle::Monospace.resolve(ui.style()),
        };
        let (slot, json) = match source.side {
            BodySide::Request => (&mut self.request, &mut self.request_json),
            BodySide::Response => (&mut self.response, &mut self.response_json),
        };
        if slot.as_ref().is_none_or(|(cached, _)| *cached != key) {
            self.builds += 1;
            // Use the keyed theme's style: after a theme switch this frame's
            // style still belongs to the previous theme.
            let text_color = ui.ctx().style_of(key.theme).visuals.text_color();
            let content = build(source, &key, text_color, json);
            *slot = Some((key, content));
        }
        if let Some((key, content)) = slot {
            render(ui, content, key);
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
    json: &mut JsonCache,
) -> Content {
    let (text, notice) = match body_view::decode(source.bytes, source.encodings, source.complete) {
        BodyText::Ready { text, notice } => (text, notice),
        BodyText::Unavailable(message) => return Content::Unavailable(message),
    };
    let format = BodyFormat::from_content_type(source.content_type);
    let palette = highlight::SyntaxPalette::for_theme(key.theme);
    let mut tree_fallback = None;
    if key.mode == ViewMode::Tree {
        if format == BodyFormat::Xml {
            match XmlTree::build(&text) {
                Ok(tree) => {
                    let view = XmlTreeView::new(tree, &palette, text_color, &key.font);
                    return Content::Tree {
                        notice,
                        view: TreeView::Xml(view),
                    };
                }
                Err(reason) => {
                    tree_fallback =
                        Some(format!("Showing Pretty: could not read a tree: {reason}."));
                }
            }
        } else if format == BodyFormat::Json {
            match json.get(source, &text) {
                Ok(tree) => {
                    return Content::Tree {
                        notice,
                        view: TreeView::Json(tree),
                    };
                }
                Err(reason) => {
                    tree_fallback =
                        Some(format!("Showing Pretty: could not read a tree: {reason}."));
                }
            }
        } else {
            tree_fallback = Some(format!(
                "Tree view is not available for {}; showing Pretty.",
                format.label()
            ));
        }
    }
    let mut formatted = body_format::format(format, key.mode, &text);
    if format == BodyFormat::Css && key.mode == ViewMode::Pretty && !source.complete {
        formatted.fallback =
            Some("Incomplete CSS preview (up to 8 KiB); showing the available prefix.".to_owned());
    }
    let numbered = matches!(format, BodyFormat::JavaScript | BodyFormat::Css);
    let partial = (format == BodyFormat::JavaScript && !source.complete).then(|| {
        "Only part of the script was captured; the view ends where the preview ends.".to_owned()
    });
    Content::Ready {
        format,
        notice,
        fallback: formatted.fallback.or(tree_fallback),
        warning: formatted.warning.or(partial),
        note: formatted.note,
        job: highlight::layout_job(&formatted.styled, &palette, text_color, &key.font),
        lines: numbered
            .then(|| highlight::line_jobs(&formatted.styled, &palette, text_color, &key.font)),
        galley: None,
    }
}

fn render(ui: &mut egui::Ui, content: &mut Content, key: &Key) {
    let (format, notice, warnings, note, job, lines, galley) = match content {
        Content::Unavailable(message) => {
            ui.colored_label(Palette::of(ui).warning, message.as_str());
            return;
        }
        Content::Tree { notice, view } => {
            if let Some(notice) = notice {
                ui.weak(notice.as_str());
            }
            view.show(ui, key);
            return;
        }
        Content::Ready {
            format,
            notice,
            fallback,
            warning,
            note,
            job,
            lines,
            galley,
        } => (
            *format,
            notice,
            [&*fallback, &*warning],
            &*note,
            &*job,
            lines.as_deref(),
            galley,
        ),
    };
    ui.weak(format!("Format: {} · {}", format.label(), key.mode.label()));
    if let Some(notice) = notice {
        ui.weak(notice.as_str());
    }
    for warning in warnings.into_iter().flatten() {
        ui.colored_label(Palette::of(ui).warning, warning.as_str());
    }
    if let Some(note) = note {
        ui.weak(note.as_str());
    }
    if let Some(lines) = lines {
        render_numbered_lines(ui, lines, key.side);
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
    ui.add(
        egui::TextEdit::multiline(&mut text)
            .font(egui::TextStyle::Monospace)
            .desired_rows(8)
            .interactive(false)
            .layouter(&mut layouter),
    );
}

/// Draws `lines` with a line number gutter, laying out only the rows that
/// are scrolled into view. Long lines scroll sideways instead of wrapping so
/// each row is one source line.
fn render_numbered_lines(ui: &mut egui::Ui, lines: &[LayoutJob], side: BodySide) {
    let font = egui::TextStyle::Monospace.resolve(ui.style());
    let row_height = ui.fonts_mut(|fonts| fonts.row_height(&font));
    let digits = lines.len().to_string().len();
    egui::ScrollArea::both()
        .id_salt(("body-lines", side))
        .max_height(LINE_VIEW_HEIGHT)
        .auto_shrink([false, true])
        .show_rows(ui, row_height, lines.len(), |ui, rows| {
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

    fn source(record: u64, bytes: &[u8]) -> BodySource<'_> {
        BodySource {
            record,
            side: BodySide::Response,
            bytes,
            encodings: &[],
            content_type: Some("application/json"),
            complete: true,
        }
    }

    #[test]
    fn css_preview_caches_gutter_and_warns_when_truncated() {
        let ctx = egui::Context::default();
        let mut cache = BodyViewCache::default();
        let css = BodySource {
            content_type: Some("text/css"),
            complete: false,
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
                fallback,
                ..
            },
        )) = &cache.response
        {
            assert_eq!(job.text, "a{\n  color:red\n}");
            let texts: Vec<_> = lines
                .as_deref()
                .unwrap_or_default()
                .iter()
                .map(|line| line.text.as_str())
                .collect();
            assert_eq!(texts, ["a{", "  color:red", "}"]);
            assert!(
                fallback
                    .as_ref()
                    .is_some_and(|s| s.contains("Incomplete CSS"))
            );
        } else {
            assert_eq!(cache.builds(), 0, "expected prepared CSS content");
        }
        run(&mut cache, &ctx, &css, ViewMode::Raw);
        if let Some((_, Content::Ready { job, lines, .. })) = &cache.response {
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
            content_type: Some("text/css"),
            encodings: &["gzip"],
            ..source(1, &compressed)
        };
        let ctx = egui::Context::default();
        let mut cache = BodyViewCache::default();
        run(&mut cache, &ctx, &css, ViewMode::Pretty);
        let Some((
            _,
            Content::Ready {
                fallback,
                notice,
                job,
                ..
            },
        )) = &cache.response
        else {
            return Err("expected prepared CSS content".into());
        };
        assert_eq!(fallback, &None);
        assert!(
            notice
                .as_ref()
                .is_some_and(|s| s.contains("first 8 KiB of decoded bytes"))
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
        let text_color = match &cache.response {
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
        let shown = |cache: &BodyViewCache| match &cache.response {
            Some((_, Content::Ready { job, .. })) => job.text.clone(),
            _ => String::new(),
        };
        run(&mut cache, &ctx, &source(1, body), ViewMode::Pretty);
        assert_eq!(shown(&cache), "{\n  \"a\": true\n}");
        run(&mut cache, &ctx, &source(1, body), ViewMode::Raw);
        assert_eq!(shown(&cache).as_bytes(), body);
    }

    #[test]
    fn json_tree_uses_shared_content_path_and_keeps_its_parse_cache() -> Result<(), String> {
        let ctx = egui::Context::default();
        let mut cache = BodyViewCache::default();
        let body = source(1, br#"{"b":1,"a":2,"b":1e2}"#);
        run(&mut cache, &ctx, &body, ViewMode::Tree);
        let first = match &cache.response {
            Some((
                _,
                Content::Tree {
                    view: TreeView::Json(tree),
                    ..
                },
            )) => Rc::clone(tree),
            _ => return Err("JSON did not use the shared Tree content path".into()),
        };
        run(&mut cache, &ctx, &body, ViewMode::Raw);
        ctx.set_theme(Theme::Light);
        run(&mut cache, &ctx, &body, ViewMode::Tree);
        assert!(
            matches!(&cache.response, Some((_, Content::Tree { view: TreeView::Json(tree), .. })) if Rc::ptr_eq(&first, tree))
        );
        run(&mut cache, &ctx, &source(2, body.bytes), ViewMode::Tree);
        assert!(
            matches!(&cache.response, Some((_, Content::Tree { view: TreeView::Json(tree), .. })) if !Rc::ptr_eq(&first, tree))
        );
        run(&mut cache, &ctx, &source(3, b"[1,"), ViewMode::Tree);
        assert!(matches!(
            &cache.response,
            Some((
                _,
                Content::Ready {
                    fallback: Some(_),
                    ..
                }
            ))
        ));
        Ok(())
    }

    #[test]
    fn tree_mode_builds_an_xml_tree_and_falls_back_for_other_formats() {
        let ctx = egui::Context::default();
        let mut cache = BodyViewCache::default();
        let xml = BodySource {
            content_type: Some("application/rss+xml"),
            ..source(1, b"<rss><channel><title>A</title></channel></rss>")
        };
        run(&mut cache, &ctx, &xml, ViewMode::Tree);
        assert!(matches!(&cache.response, Some((_, Content::Tree { .. }))));
        let text = BodySource {
            content_type: Some("text/plain"),
            ..source(2, b"plain text")
        };
        run(&mut cache, &ctx, &text, ViewMode::Tree);
        let fallback = match &cache.response {
            Some((_, Content::Ready { fallback, job, .. })) => {
                Some((fallback.clone(), job.text.clone()))
            }
            _ => None,
        };
        assert_eq!(
            fallback,
            Some((
                Some("Tree view is not available for Text; showing Pretty.".to_owned()),
                "plain text".to_owned()
            ))
        );
    }

    #[test]
    fn numbers_script_lines_and_warns_about_partial_capture() {
        let ctx = egui::Context::default();
        let mut cache = BodyViewCache::default();
        let script = BodySource {
            content_type: Some("application/javascript"),
            complete: false,
            ..source(1, b"f(function(){a();b()}); x()")
        };
        run(&mut cache, &ctx, &script, ViewMode::Pretty);
        let (lines, warning) = match &cache.response {
            Some((_, Content::Ready { lines, warning, .. })) => (lines.as_deref(), warning.clone()),
            _ => (None, None),
        };
        let texts: Vec<_> = lines
            .unwrap_or_default()
            .iter()
            .map(|line| line.text.as_str())
            .collect();
        assert_eq!(texts, ["f(function(){", "  a();", "  b()", "});", "x()"]);
        assert!(warning.is_some_and(|warning| warning.contains("Only part")));
        run(&mut cache, &ctx, &script, ViewMode::Raw);
        assert!(matches!(
            &cache.response,
            Some((_, Content::Ready { lines: Some(lines), .. })) if lines.len() == 1
        ));
        run(
            &mut cache,
            &ctx,
            &source(2, br#"{"a":1}"#),
            ViewMode::Pretty,
        );
        assert!(matches!(
            &cache.response,
            Some((_, Content::Ready { lines: None, .. }))
        ));
    }
}
