//! The Body tab: the view mode, then one section per body with a chrome
//! header (name, format, size, and its Save menu) over the raw viewer.

use eframe::egui;
use http_inspector_content::detect;
use http_inspector_content::format::{BodyFormat, ViewMode};
use http_inspector_exchange::{
    BodyPreview, BodyRetention, BodySide, BodySnapshot, ByteSize, Exchange, HeaderPreview,
    ImageFormat, MediaType, Outcome, RecordId,
};

use crate::ui::AppAction;
use crate::ui::detail::body_cache::{BodySource, Capture};
use crate::ui::detail::body_render::BodyViewCache;
use crate::ui::detail::image_view;
use crate::ui::detail::search::Query;
use crate::ui::dialogs::body_saving::BodySaving;
use crate::ui::i18n::{Language, domain};
use crate::ui::style::fonts;
use crate::ui::style::indicator;
use crate::ui::style::theme::Palette;
use crate::ui::style::tokens::{Layout, Space, StrokeWidth, Tone};
use crate::ui::workbench::text::ShellText;
use crate::ui::workbench::{chrome, controls};

/// Where and how the bodies of the selected exchange are shown.
pub(super) struct BodyPanel<'a> {
    pub(super) record: RecordId,
    pub(super) cache: &'a mut BodyViewCache,
    pub(super) image: &'a mut image_view::ImagePanel,
    pub(super) saving: &'a BodySaving,
    /// How text bodies are shown.
    pub(super) mode: ViewMode,
    /// The active search, whose matches are marked.
    pub(super) search: Option<&'a Query>,
    /// Text the user chose to copy from a tree.
    pub(super) copied: Option<String>,
    pub(super) lang: Language,
}

pub(super) fn show(
    ui: &mut egui::Ui,
    record: &Exchange,
    panel: &mut BodyPanel<'_>,
    actions: &mut Vec<AppAction>,
) {
    let request = record.request();
    let request_body = request
        .body()
        .map(|body| Body::new(body, request.headers(), request.media_type()));
    let response = match record.outcome() {
        Outcome::Response(response) => Some(response),
        _ => None,
    };
    let response_body = response
        .map(|response| Body::new(response.body(), response.headers(), response.media_type()));
    let mode_bodies: Vec<&Body<'_>> = [request_body.as_ref(), response_body.as_ref()]
        .into_iter()
        .flatten()
        .filter(|body| body.uses_view_mode())
        .collect();
    if !mode_bodies.is_empty() {
        let has_tree = mode_bodies.iter().any(|body| has_tree(body.media_type));
        view_mode_row(ui, &mut panel.mode, has_tree, panel.lang);
    }
    actions.extend(section(
        ui,
        record,
        panel,
        BodySide::Request,
        request_body.as_ref(),
    ));
    if response.is_some() {
        actions.extend(section(
            ui,
            record,
            panel,
            BodySide::Response,
            response_body.as_ref(),
        ));
    }
}

/// One body as captured.
struct Body<'a> {
    snapshot: Option<BodySnapshot>,
    headers: &'a HeaderPreview,
    media_type: Option<&'a MediaType>,
}

impl<'a> Body<'a> {
    fn new(
        body: &BodyPreview,
        headers: &'a HeaderPreview,
        media_type: Option<&'a MediaType>,
    ) -> Self {
        Self {
            snapshot: body.snapshot(),
            headers,
            media_type,
        }
    }

    fn image(&self) -> Option<ImageFormat> {
        self.media_type.and_then(ImageFormat::from_media_type)
    }

    fn uses_view_mode(&self) -> bool {
        self.image().is_none()
            && self.snapshot.as_ref().is_some_and(|snapshot| {
                snapshot.retention() != BodyRetention::Omitted && !snapshot.bytes().is_empty()
            })
    }
}

/// Whether a body of `media_type` may be shown as a tree: its `Content-Type`
/// names JSON or XML, or does not name a format and the body may sniff as JSON.
fn has_tree(media_type: Option<&MediaType>) -> bool {
    detect::from_content_type(media_type)
        .is_none_or(|detection| matches!(detection.format, BodyFormat::Json | BodyFormat::Xml))
}

/// Pretty, Raw, and Tree as Ghost toggles. Tree is enabled only when a body
/// may be JSON or XML.
fn view_mode_row(ui: &mut egui::Ui, mode: &mut ViewMode, has_tree: bool, lang: Language) {
    if !has_tree && *mode == ViewMode::Tree {
        *mode = ViewMode::Pretty;
    }
    egui::Frame::new()
        .inner_margin(Layout::PANE_ROW_MARGIN)
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                controls::ghost_style(ui);
                ui.spacing_mut().item_spacing.x = Space::XS;
                for option in ViewMode::ALL {
                    let enabled = option != ViewMode::Tree || has_tree;
                    let response = ui
                        .add_enabled_ui(enabled, |ui| {
                            controls::ghost_toggle(
                                ui,
                                *mode == option,
                                domain::view_mode(lang, option),
                                Layout::PANE_BUTTON_HEIGHT,
                            )
                        })
                        .inner
                        .on_disabled_hover_text(ShellText::TreeUnavailable.text(lang));
                    if response.clicked() {
                        *mode = option;
                    }
                }
            });
        });
}

/// The header of one body section and its content under it. Returns the
/// action picked from the section's Save menu.
fn section(
    ui: &mut egui::Ui,
    record: &Exchange,
    panel: &mut BodyPanel<'_>,
    side: BodySide,
    body: Option<&Body<'_>>,
) -> Option<AppAction> {
    let mode = panel.mode;
    let lang = panel.lang;
    let source = body.and_then(|body| {
        let snapshot = body.snapshot.as_ref()?;
        (snapshot.retention() != BodyRetention::Omitted
            && !snapshot.bytes().is_empty()
            && body.image().is_none())
        .then_some((snapshot, body))
    });
    let encodings: Vec<&str> = body.map_or_else(Vec::new, |body| {
        body.headers.get("content-encoding").collect()
    });
    let format = source.and_then(|(snapshot, body)| {
        panel.cache.prepare(
            ui,
            &body_source(panel, side, snapshot, body, &encodings),
            mode,
            lang,
        )
    });
    let action = section_header(ui, record, panel, side, body, format);
    let Some(body) = body else {
        return action;
    };
    let Some(snapshot) = body.snapshot.as_ref() else {
        return action;
    };
    if snapshot.retention() == BodyRetention::Omitted || snapshot.bytes().is_empty() {
        return action;
    }
    if let Some(format) = body.image() {
        egui::Frame::new()
            .inner_margin(Layout::PANE_ROW_MARGIN)
            .show(ui, |ui| {
                panel.image.show(
                    ui,
                    &image_view::ImageBody {
                        record: panel.record,
                        snapshot,
                        encodings: &encodings,
                        declared: format,
                    },
                    lang,
                );
            });
        return action;
    }
    if let Some(text) = panel.cache.render(ui, side, mode, lang) {
        panel.copied = Some(text);
    }
    action
}

fn body_source<'a>(
    panel: &BodyPanel<'a>,
    side: BodySide,
    snapshot: &'a BodySnapshot,
    body: &Body<'a>,
    encodings: &'a [&'a str],
) -> BodySource<'a> {
    BodySource {
        record: panel.record.value(),
        side,
        bytes: snapshot.bytes(),
        encodings,
        media_type: body.media_type,
        capture: Capture::of(snapshot),
        search: panel.search.filter(|query| query.scope.covers_body(side)),
    }
}

/// A 28pt chrome band: the section name, then its format and size in
/// monospace `text.secondary`, and the Save menu at the right (D22).
fn section_header(
    ui: &mut egui::Ui,
    record: &Exchange,
    panel: &BodyPanel<'_>,
    side: BodySide,
    body: Option<&Body<'_>>,
    format: Option<BodyFormat>,
) -> Option<AppAction> {
    let lang = panel.lang;
    let title = match side {
        BodySide::Request => ShellText::RequestBody,
        BodySide::Response => ShellText::ResponseBody,
    };
    let top = ui.cursor().top();
    let action = chrome::pane_header(ui, side_salt(side), |ui| {
        ui.label(
            egui::RichText::new(title.text(lang))
                .font(fonts::panel_title())
                .color(Palette::of(ui).text),
        );
        meta(ui, body, format, lang);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            panel.saving.menu(ui, record, side, lang)
        })
        .inner
    });
    // The band above it is the view mode row or another section.
    ui.painter().hline(
        ui.max_rect().x_range(),
        top + StrokeWidth::STRUCTURAL / 2.0,
        egui::Stroke::new(StrokeWidth::STRUCTURAL, Palette::of(ui).borders.structural),
    );
    action
}

const fn side_salt(side: BodySide) -> &'static str {
    match side {
        BodySide::Request => "body-section-request",
        BodySide::Response => "body-section-response",
    }
}

/// The format and size of a body, or why there is nothing to show.
fn meta(ui: &mut egui::Ui, body: Option<&Body<'_>>, format: Option<BodyFormat>, lang: Language) {
    let secondary = Palette::of(ui).secondary;
    let data = |ui: &mut egui::Ui, text: &str| {
        ui.label(
            egui::RichText::new(text)
                .font(fonts::status())
                .color(secondary),
        );
    };
    let Some(body) = body else {
        data(ui, ShellText::NotObserved.text(lang));
        return;
    };
    let Some(snapshot) = body.snapshot.as_ref() else {
        data(ui, ShellText::Updating.text(lang));
        return;
    };
    if let Some(format) = format {
        data(ui, domain::body_format(lang, format));
    }
    data(ui, &ByteSize::new(snapshot.observed_bytes()).to_string());
    if snapshot.retention() == BodyRetention::Omitted {
        indicator::dot_label(ui, Tone::Warning, ShellText::PreviewOmitted.text(lang))
            .on_hover_text(ShellText::PreviewOmittedHint.text(lang));
    }
}
