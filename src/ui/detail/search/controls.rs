use std::collections::BTreeSet;
use std::sync::Arc;

use eframe::egui;
use http_inspector_analysis::filter::{FilterDiagnostic, FilterKey, RecordList, TargetParts};
use http_inspector_analysis::search::{MatchOptions, SearchScope};
use http_inspector_exchange::Exchange;

use super::{RequestSearch, SearchSettings};
use crate::ui::i18n::{Language, domain};
use crate::ui::requests::filter_menu::field_term;
use crate::ui::style::{
    fonts,
    tokens::{Layout, Space, StrokeWidth, ThemeTokens},
};
use crate::ui::workbench::controls;
use crate::ui::workbench::field::{self, Field};
use crate::ui::workbench::text::{self, ShellText};

const MAX_SUGGESTIONS: usize = 32;

const FOCUS_FILTER: egui::KeyboardShortcut =
    egui::KeyboardShortcut::new(egui::Modifiers::COMMAND, egui::Key::F);

impl RequestSearch {
    pub(crate) fn controls(
        &self,
        ui: &mut egui::Ui,
        width: f32,
        records: &RecordList<Exchange>,
        dialog_open: bool,
        lang: Language,
    ) -> Option<SearchSettings> {
        let mut settings = self.settings().clone();
        let diagnostics = self.parsed().diagnostics.as_slice();
        let regex = settings.options.regex;
        let id = ui.make_persistent_id("request-filter");
        if !dialog_open && ui.input_mut(|input| input.consume_shortcut(&FOCUS_FILTER)) {
            ui.memory_mut(|memory| memory.request_focus(id));
        }
        let mut clear = false;
        let can_clear = !settings.text.is_empty();
        let field = Field::new(width)
            .hint(if regex {
                ShellText::FilterRegex.text(lang)
            } else {
                ShellText::Filter.text(lang)
            })
            .search_icon()
            .monospace()
            .error(if self.regex_error().is_some() {
                Some(ShellText::InvalidFilter.text(lang))
            } else {
                (!diagnostics.is_empty()).then_some(ShellText::InvalidCondition.text(lang))
            });
        let mut layouter = |ui: &egui::Ui, buffer: &dyn egui::TextBuffer, _| {
            let text = buffer.as_str();
            let errors = if text == self.settings().text {
                diagnostics
            } else {
                &[]
            };
            layout_filter_text(ui, text, errors)
        };
        let layouter =
            (!diagnostics.is_empty()).then_some(&mut layouter as &mut field::Layouter<'_>);
        let response = field::single_line_with(
            ui,
            &mut settings.text,
            field,
            |ui| {
                let height = 2.0f32.mul_add(-Space::XS, Layout::INPUT_HEIGHT);
                clear = ui
                    .add_enabled(can_clear, controls::ghost("×", height))
                    .accessible_name(ShellText::ClearFilter.text(lang))
                    .on_hover_text(ShellText::ClearFilter.text(lang))
                    .clicked();
                filter_help(ui, height, lang);
                options(ui, &mut settings.scope, &mut settings.options, lang);
            },
            layouter,
            |ui, edit| {
                egui::CompletionPopup::new(id)
                    .show(ui, edit, |query| {
                        if regex {
                            Vec::new()
                        } else {
                            suggestions(query, records)
                        }
                    })
                    .text_edit
                    .response
                    .response
            },
        );
        if clear {
            settings.text.clear();
        }
        self.filter_tooltip(ui, response, diagnostics, lang);
        (&settings != self.settings()).then_some(settings)
    }
    fn filter_tooltip(
        &self,
        ui: &egui::Ui,
        response: egui::Response,
        diagnostics: &[FilterDiagnostic],
        lang: Language,
    ) {
        let response = response.accessible_name(ShellText::FilterRequests.text(lang));
        if let Some(error) = self.regex_error() {
            response.on_hover_text(domain::matcher_error(lang, error));
        } else if diagnostics.is_empty() {
            response.on_hover_text(text::focus_filter(
                lang,
                &ui.ctx().format_shortcut(&FOCUS_FILTER),
            ));
        } else {
            response.on_hover_ui(|ui| {
                for diagnostic in diagnostics {
                    ui.label(domain::localize_filter_diagnostic(
                        lang,
                        &diagnostic.message,
                    ));
                }
            });
        }
    }
}

fn layout_filter_text(
    ui: &egui::Ui,
    text: &str,
    diagnostics: &[FilterDiagnostic],
) -> Arc<egui::Galley> {
    let theme = ThemeTokens::for_theme(ui.ctx().theme());
    let error_stroke = egui::Stroke::new(StrokeWidth::STRUCTURAL, theme.status.error);
    let format = egui::TextFormat {
        font_id: fonts::data(),
        color: theme.text.primary,
        ..Default::default()
    };
    let mut job = egui::text::LayoutJob::default();
    let mut start = 0;
    for diagnostic in diagnostics {
        if let (Some(prefix), Some(term)) = (
            text.get(start..diagnostic.span.start),
            text.get(diagnostic.span.clone()),
        ) {
            job.append(prefix, 0.0, format.clone());
            job.append(
                term,
                0.0,
                egui::TextFormat {
                    underline: error_stroke,
                    ..format.clone()
                },
            );
            start = diagnostic.span.end;
        }
    }
    job.append(text.get(start..).unwrap_or_default(), 0.0, format);
    ui.fonts_mut(|fonts| fonts.layout_job(job))
}

fn filter_help(ui: &mut egui::Ui, height: f32, lang: Language) {
    controls::ghost_menu(ui, "?", height, |ui| {
        for (keys, example) in [
            ("method", "method:GET,POST"),
            ("url, path", "url:/api path:?page="),
            ("host, domain", "host:*.example.com"),
            ("status", "status:4xx,>=500"),
            ("type", "type:json"),
            (
                "header, req-header, res-header",
                "header:\"content-type=application/json\"",
            ),
            ("size", "size:>10kb"),
            ("duration, time", "duration:>500ms"),
            ("is", "is:failed,pending,tunnel,replay,https"),
            (ShellText::Exclude.text(lang), "-url:/static/"),
            (ShellText::Phrase.text(lang), "\"connection refused\""),
        ] {
            ui.horizontal(|ui| {
                ui.label(keys);
                ui.monospace(example);
            });
        }
    })
    .response
    .accessible_name(ShellText::FilterSyntax.text(lang))
    .on_hover_text(ShellText::FilterSyntax.text(lang));
}

/// Search options inside the right end of the filter input: scope, case,
/// and regular expression.
fn options(
    ui: &mut egui::Ui,
    scope: &mut SearchScope,
    match_options: &mut MatchOptions,
    lang: Language,
) {
    controls::ghost_scope(ui, |ui| {
        let height = 2.0f32.mul_add(-Space::XS, Layout::INPUT_HEIGHT);
        toggle(
            ui,
            &mut match_options.regex,
            ".*",
            ShellText::UseRegex.text(lang),
            height,
        );
        toggle(
            ui,
            &mut match_options.case_sensitive,
            "Aa",
            ShellText::MatchCase.text(lang),
            height,
        );
        egui::containers::menu::MenuButton::from_button(controls::ghost(
            text::menu_label(domain::search_scope(lang, *scope)),
            height,
        ))
        .ui(ui, |ui| {
            for option in SearchScope::ALL {
                if ui
                    .selectable_value(scope, option, domain::search_scope(lang, option))
                    .clicked()
                {
                    ui.close();
                }
            }
        })
        .0
        .on_hover_text(ShellText::SearchScope.text(lang));
    });
}

fn toggle(ui: &mut egui::Ui, value: &mut bool, label: &str, hover: &str, height: f32) {
    if controls::ghost_toggle(ui, *value, label, height)
        .on_hover_text(hover)
        .clicked()
    {
        *value = !*value;
    }
}

fn suggestions(
    query: &egui::CompletionQuery<'_>,
    records: &RecordList<Exchange>,
) -> Vec<egui::Suggestion> {
    if !query.word.ends_with(':') {
        return Vec::new();
    }
    // egui counts characters for its replacement range. The filter lexer uses UTF-8 offsets.
    let Some((start, _)) = query.text.char_indices().nth(query.word_range.start.0) else {
        return Vec::new();
    };
    let span = start..start + query.word.len();
    let Some((key, excluded)) = FilterKey::for_completion(query.text, span) else {
        return Vec::new();
    };
    let candidates: BTreeSet<_> = match key {
        FilterKey::Method => records
            .iter()
            .map(|record| record.request().method().as_str())
            .collect(),
        FilterKey::Host => records
            .iter()
            .map(|record| TargetParts::of(record.request().target()).host)
            .filter(|host| !host.is_empty())
            .collect(),
        FilterKey::Status => ["2xx", "4xx", "5xx"].into_iter().collect(),
        _ => return Vec::new(),
    };
    candidates
        .into_iter()
        .take(MAX_SUGGESTIONS)
        .map(|value| egui::Suggestion::new(format!("{} ", field_term(key, value, excluded))))
        .collect()
}
