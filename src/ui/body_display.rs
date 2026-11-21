//! Decoded, formatted, and highlighted body text for the detail panel,
//! rebuilt only when the shown body or the way it is shown changes.

use crate::ui::body_format::xml::tree::XmlTree;
use crate::ui::body_format::{self, BodyFormat, ViewMode, highlight};
use crate::ui::body_view::{self, BodyText};
use crate::ui::theme::{Palette, Theme};
use crate::ui::xml_tree_view::XmlTreeView;
use eframe::egui::{self, Galley, text::LayoutJob};
use std::sync::Arc;

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
        view: XmlTreeView,
    },
    Ready {
        format: BodyFormat,
        notice: Option<String>,
        fallback: Option<String>,
        warning: Option<String>,
        job: LayoutJob,
        /// The last laid-out galley and the wrap width it was laid out for.
        galley: Option<(u32, Arc<Galley>)>,
    },
}

/// Highlighted text of the request and response bodies last shown.
#[derive(Default)]
pub(in crate::ui) struct BodyViewCache {
    request: Option<(Key, Content)>,
    response: Option<(Key, Content)>,
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
            complete: source.complete,
            mode,
            theme: ui.ctx().theme(),
            font: egui::TextStyle::Monospace.resolve(ui.style()),
        };
        let slot = match source.side {
            BodySide::Request => &mut self.request,
            BodySide::Response => &mut self.response,
        };
        if slot.as_ref().is_none_or(|(cached, _)| *cached != key) {
            self.builds += 1;
            // Use the keyed theme's style: after a theme switch this frame's
            // style still belongs to the previous theme.
            let text_color = ui.ctx().style_of(key.theme).visuals.text_color();
            let content = build(source, &key, text_color);
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

fn build(source: &BodySource<'_>, key: &Key, text_color: egui::Color32) -> Content {
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
                    return Content::Tree { notice, view };
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
    let formatted = body_format::format(format, key.mode, &text);
    Content::Ready {
        format,
        notice,
        fallback: formatted.fallback.or(tree_fallback),
        warning: formatted.warning,
        job: highlight::layout_job(&formatted.styled, &palette, text_color, &key.font),
        galley: None,
    }
}

fn render(ui: &mut egui::Ui, content: &mut Content, key: &Key) {
    let (format, notice, warnings, job, galley) = match content {
        Content::Unavailable(message) => {
            ui.colored_label(Palette::of(ui).warning, message.as_str());
            return;
        }
        Content::Tree { notice, view } => {
            ui.weak(format!(
                "Format: {} · {}",
                BodyFormat::Xml.label(),
                key.mode.label()
            ));
            if let Some(notice) = notice {
                ui.weak(notice.as_str());
            }
            if let Some(incomplete) = view.incomplete() {
                ui.colored_label(Palette::of(ui).warning, incomplete);
            }
            view.show(ui, ("xml-tree", key.record, key.side));
            return;
        }
        Content::Ready {
            format,
            notice,
            fallback,
            warning,
            job,
            galley,
        } => (*format, notice, [&*fallback, &*warning], &*job, galley),
    };
    ui.weak(format!("Format: {} · {}", format.label(), key.mode.label()));
    if let Some(notice) = notice {
        ui.weak(notice.as_str());
    }
    for warning in warnings.into_iter().flatten() {
        ui.colored_label(Palette::of(ui).warning, warning.as_str());
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
    fn tree_mode_builds_an_xml_tree_and_falls_back_for_other_formats() {
        let ctx = egui::Context::default();
        let mut cache = BodyViewCache::default();
        let xml = BodySource {
            content_type: Some("application/rss+xml"),
            ..source(1, b"<rss><channel><title>A</title></channel></rss>")
        };
        run(&mut cache, &ctx, &xml, ViewMode::Tree);
        assert!(matches!(&cache.response, Some((_, Content::Tree { .. }))));
        run(&mut cache, &ctx, &source(2, br#"{"a":1}"#), ViewMode::Tree);
        let fallback = match &cache.response {
            Some((_, Content::Ready { fallback, job, .. })) => {
                Some((fallback.clone(), job.text.clone()))
            }
            _ => None,
        };
        assert_eq!(
            fallback,
            Some((
                Some("Tree view is not available for JSON; showing Pretty.".to_owned()),
                "{\n  \"a\": 1\n}".to_owned()
            ))
        );
    }
}
