//! The inspector pane: tabs, the request line, and the tab the user picked.

use crate::ui::AppAction;
use crate::ui::detail::body_render::BodyViewCache;
use crate::ui::detail::body_tab::{self, BodyPanel};
use crate::ui::detail::detail_tabs::{self, DetailTab};
use crate::ui::detail::search::{Query, highlight};
use crate::ui::detail::{headers_tab, image_view, timing_tab};
use crate::ui::dialogs::{body_saving::BodySaving, copy_curl};
use crate::ui::i18n::Language;
use crate::ui::requests::outcome_view;
use crate::ui::style::fonts;
use crate::ui::style::theme::{MatchColors, Palette};
use crate::ui::style::tokens::{Layout, Space, StrokeWidth};
use crate::ui::workbench::text::{self, ShellText};
use crate::ui::workbench::{chrome, controls};
use eframe::egui;
use http_inspector_content::format::ViewMode;
use http_inspector_exchange::{CaptureSource, Exchange, RecordId};

#[derive(Default)]
pub(crate) struct DetailWidgets {
    body: BodyViewCache,
    image: image_view::ImagePanel,
}

pub(crate) struct DetailView<'a> {
    pub(crate) record: Option<&'a Exchange>,
    pub(crate) search: Option<&'a Query>,
    pub(crate) tab: DetailTab,
    pub(crate) body_mode: ViewMode,
    /// Header rows offer filter terms in their context menu.
    pub(crate) field_menu_enabled: bool,
    pub(crate) body_saving: &'a BodySaving,
    pub(crate) lang: Language,
}

impl DetailView<'_> {
    pub(crate) fn show(
        &self,
        ui: &mut egui::Ui,
        widgets: &mut DetailWidgets,
        shortcuts_allowed: bool,
        actions: &mut Vec<AppAction>,
    ) {
        let lang = self.lang;
        let mut tab = self.tab;
        let Some(record) = self.record else {
            chrome::pane_header(ui, "details-header", |ui| {
                detail_tabs::show(ui, &mut tab, lang)
            });
            ui.centered_and_justified(|ui| {
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(ShellText::NoRequestSelected.text(lang)).weak(),
                    )
                    .wrap(),
                );
            });
            if tab != self.tab {
                actions.push(AppAction::DetailTab(tab));
            }
            return;
        };
        if shortcuts_allowed {
            detail_tabs::apply_shortcuts(ui, &mut tab);
        }
        let search = self.search;
        header(ui, record, search, &mut tab, lang, actions);
        if tab != self.tab {
            actions.push(AppAction::DetailTab(tab));
        }
        let mut body_mode = self.body_mode;

        let (copied, filter) = egui::ScrollArea::vertical()
            .id_salt(("details-scroll", tab))
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                match tab {
                    DetailTab::Headers => {
                        let filter = headers_tab::show(
                            ui,
                            record,
                            search.filter(|query| query.scope.covers_headers()),
                            self.field_menu_enabled,
                            lang,
                        );
                        (None, filter)
                    }
                    DetailTab::Body => {
                        let mut panel = BodyPanel {
                            record: record.id(),
                            cache: &mut widgets.body,
                            image: &mut widgets.image,
                            saving: self.body_saving,
                            mode: body_mode,
                            search,
                            copied: None,
                            lang,
                        };
                        body_tab::show(ui, record, &mut panel, actions);
                        body_mode = panel.mode;
                        (panel.copied, None)
                    }
                    DetailTab::Timing => {
                        timing_tab::show(ui, record, lang);
                        (None, None)
                    }
                }
            })
            .inner;
        actions.extend(copied.map(AppAction::CopyText));
        actions.extend(filter);
        if body_mode != self.body_mode {
            actions.push(AppAction::BodyMode(body_mode));
        }
    }
}

/// The pane header with the tabs and, when they fit, the record actions,
/// then the request line and outcome of `record` under it.
fn header(
    ui: &mut egui::Ui,
    record: &Exchange,
    search: Option<&Query>,
    tab: &mut DetailTab,
    lang: Language,
    actions: &mut Vec<AppAction>,
) {
    let actions_in_header = chrome::pane_header(ui, "details-header", |ui| {
        detail_tabs::show(ui, tab, lang);
        let fits = ui.available_width() >= header_actions_width(ui, lang) + Space::LG;
        if fits {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                header_actions(ui, record, lang, actions);
            });
        }
        fits
    });
    let title = egui::Frame::new()
        .inner_margin(Layout::REQUEST_LINE_MARGIN)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = Space::XS;
            request_line(ui, record, search);
            summary_row(ui, record, actions_in_header, lang, actions);
        });
    ui.painter().hline(
        ui.max_rect().x_range(),
        title.response.rect.bottom() - StrokeWidth::STRUCTURAL / 2.0,
        egui::Stroke::new(StrokeWidth::STRUCTURAL, Palette::of(ui).borders.subtle),
    );
}

/// Edit and Resend and Copy as cURL, laid out right to left in the pane
/// header, or left to right under the request line when the header is too
/// narrow.
fn header_actions(
    ui: &mut egui::Ui,
    record: &Exchange,
    lang: Language,
    actions: &mut Vec<AppAction>,
) {
    controls::ghost_style(ui);
    let resend = |ui: &mut egui::Ui, actions: &mut Vec<AppAction>| {
        let button = controls::ghost(
            ShellText::EditAndResend.text(lang),
            Layout::PANE_BUTTON_HEIGHT,
        );
        if ui
            .add_enabled(record.source() != CaptureSource::Connect, button)
            .clicked()
        {
            actions.push(AppAction::OpenReplay(record.id()));
        }
    };
    let copy = |ui: &mut egui::Ui, actions: &mut Vec<AppAction>| {
        if copy_curl::copy_button(ui, record, lang).clicked() {
            actions.push(AppAction::CopyCurl(record.id()));
        }
    };
    if ui.layout().prefer_right_to_left() {
        copy(ui, actions);
        resend(ui, actions);
    } else {
        resend(ui, actions);
        copy(ui, actions);
    }
}

/// Width the header actions need, so the header can tell whether they fit.
fn header_actions_width(ui: &egui::Ui, lang: Language) -> f32 {
    [
        ShellText::EditAndResend.text(lang),
        copy_curl::copy_label(lang),
    ]
    .iter()
    .map(|label| controls::button_width(ui, label) + ui.spacing().item_spacing.x)
    .sum()
}

/// The outcome and, for a resent request, a link to the original. Holds the
/// header actions too when the header has no room for them.
fn summary_row(
    ui: &mut egui::Ui,
    record: &Exchange,
    actions_in_header: bool,
    lang: Language,
    actions: &mut Vec<AppAction>,
) {
    ui.horizontal_wrapped(|ui| {
        let (status, tone) = outcome_view::detail_status(record.outcome(), lang);
        ui.label(
            egui::RichText::new(status)
                .font(fonts::data())
                .color(Palette::of(ui).status(tone)),
        );
        if !actions_in_header {
            header_actions(ui, record, lang, actions);
        }
        if let Some(original) = record.replay_of()
            && ui.link(replay_link(original, lang)).clicked()
        {
            actions.push(AppAction::SelectRecord(original));
        }
    });
}

/// The method in `text.secondary` and the URL in monospace `text.primary`,
/// with URL matches of the active search marked.
fn request_line(ui: &mut egui::Ui, record: &Exchange, search: Option<&Query>) {
    let request = record.request();
    let palette = Palette::of(ui);
    let font = fonts::data();
    let mut job = egui::text::LayoutJob::default();
    job.append(
        request.method().as_str(),
        0.0,
        egui::TextFormat::simple(font.clone(), palette.secondary),
    );
    let target = match search.filter(|query| query.scope.covers_url()) {
        Some(query) => highlight::plain(
            request.target(),
            &query.terms,
            font,
            palette.text,
            MatchColors::for_theme(ui.ctx().theme()),
        ),
        None => egui::text::LayoutJob::simple_singleline(
            request.target().to_owned(),
            font,
            palette.text,
        ),
    };
    let offset = job.text.len();
    job.text.push_str(&target.text);
    for (index, mut section) in target.sections.into_iter().enumerate() {
        section.byte_range = egui::text::ByteIndex(section.byte_range.start.0 + offset)
            ..egui::text::ByteIndex(section.byte_range.end.0 + offset);
        if index == 0 {
            section.leading_space = Space::SM;
        }
        job.sections.push(section);
    }
    job.wrap.max_width = ui.available_width();
    ui.add(egui::Label::new(job).wrap());
}

/// Link to the request a replay was sent from. Links keep the text color
/// and are told apart by their underline.
fn replay_link(original: RecordId, lang: Language) -> egui::RichText {
    egui::RichText::new(text::replay_of(lang, original.value())).underline()
}
