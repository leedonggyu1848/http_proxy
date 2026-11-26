use std::collections::BTreeSet;

use eframe::egui;
use http_inspector_analysis::filter::{FilterKey, RecordList, TargetParts};
use http_inspector_analysis::search::{MatchOptions, SearchScope};
use http_inspector_exchange::Exchange;

use super::{RequestSearch, SearchSettings};
use crate::ui::requests::filter_menu::field_term;
use crate::ui::style::tokens::{Layout, Space, StrokeWidth, ThemeTokens};
use crate::ui::workbench::controls;
use crate::ui::workbench::field::{self, Field};
use crate::ui::workbench::text::ShellText;

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
                "Filter with regex"
            } else {
                ShellText::Filter.text()
            })
            .search_icon()
            .monospace()
            .error(if self.regex_error().is_some() {
                Some(ShellText::InvalidFilter.text())
            } else {
                (!diagnostics.is_empty()).then_some(ShellText::InvalidCondition.text())
            });
        let response = field::single_line_with(
            ui,
            &mut settings.text,
            field,
            |ui| {
                let height = 2.0f32.mul_add(-Space::XS, Layout::INPUT_HEIGHT);
                clear = ui
                    .add_enabled(can_clear, controls::ghost("×", height))
                    .accessible_name("Clear filter")
                    .on_hover_text("Clear filter")
                    .clicked();
                filter_help(ui, height);
                options(ui, &mut settings.scope, &mut settings.options);
            },
            |ui, edit| self.show_edit(ui, edit, id, regex, records),
        );
        if clear {
            settings.text.clear();
        }
        let response = response.accessible_name("Filter requests");
        if let Some(error) = self.regex_error() {
            response.on_hover_text(error);
        } else if diagnostics.is_empty() {
            response.on_hover_text(format!(
                "Focus filter ({})",
                ui.ctx().format_shortcut(&FOCUS_FILTER)
            ));
        } else {
            response.on_hover_ui(|ui| {
                for diagnostic in diagnostics {
                    ui.label(&diagnostic.message);
                }
            });
        }
        (&settings != self.settings()).then_some(settings)
    }

    fn show_edit(
        &self,
        ui: &mut egui::Ui,
        edit: egui::TextEdit<'_>,
        id: egui::Id,
        regex: bool,
        records: &RecordList<Exchange>,
    ) -> egui::Response {
        let output = egui::CompletionPopup::new(id)
            .show(ui, edit, |query| {
                if regex {
                    Vec::new()
                } else {
                    suggestions(query, records)
                }
            })
            .text_edit;
        self.underline_errors(ui, &output);
        output.response.response
    }

    fn underline_errors(&self, ui: &egui::Ui, output: &egui::text_edit::TextEditOutput) {
        let text = output.galley.text();
        if text != self.settings().text {
            return;
        }
        let theme = ThemeTokens::for_theme(ui.ctx().theme());
        let stroke = egui::Stroke::new(StrokeWidth::STRUCTURAL, theme.status.error);
        let painter = ui.painter().with_clip_rect(output.text_clip_rect);
        let position = |byte| {
            let index = text.get(..byte).unwrap_or_default().chars().count();
            let cursor = egui::text::CCursor::new(index);
            output
                .galley
                .pos_from_cursor(cursor)
                .translate(output.galley_pos.to_vec2())
        };
        for error in &self.parsed().diagnostics {
            let start = position(error.span.start);
            let end = position(error.span.end);
            painter.hline(start.left()..=end.left(), start.bottom(), stroke);
        }
    }
}

fn filter_help(ui: &mut egui::Ui, height: f32) {
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
            ("Exclude", "-url:/static/"),
            ("Phrase", "\"connection refused\""),
        ] {
            ui.horizontal(|ui| {
                ui.label(keys);
                ui.monospace(example);
            });
        }
    })
    .response
    .accessible_name("Filter syntax")
    .on_hover_text("Filter syntax");
}

/// Search options inside the right end of the filter input: scope, case,
/// and regular expression.
fn options(ui: &mut egui::Ui, scope: &mut SearchScope, match_options: &mut MatchOptions) {
    controls::ghost_scope(ui, |ui| {
        let height = 2.0f32.mul_add(-Space::XS, Layout::INPUT_HEIGHT);
        toggle(
            ui,
            &mut match_options.regex,
            ".*",
            "Use a regular expression",
            height,
        );
        toggle(
            ui,
            &mut match_options.case_sensitive,
            "Aa",
            "Match free-text case",
            height,
        );
        egui::containers::menu::MenuButton::from_button(controls::ghost(
            format!("{} ▼", scope.label()),
            height,
        ))
        .ui(ui, |ui| {
            for option in SearchScope::ALL {
                if ui.selectable_value(scope, option, option.label()).clicked() {
                    ui.close();
                }
            }
        })
        .0
        .on_hover_text("Search scope for free text");
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
    let (word, excluded) = query
        .word
        .strip_prefix('-')
        .map_or((query.word, false), |word| (word, true));
    let Some((key, "")) = word.split_once(':') else {
        return Vec::new();
    };
    let key = match key.to_ascii_lowercase().as_str() {
        "method" => FilterKey::Method,
        "host" | "domain" => FilterKey::Host,
        "status" => FilterKey::Status,
        _ => return Vec::new(),
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
