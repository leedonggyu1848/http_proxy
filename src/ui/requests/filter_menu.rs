use eframe::egui;
use http_inspector_analysis::filter::FilterKey;

use crate::ui::AppAction;
use crate::ui::i18n::Language;

pub(crate) fn items(
    ui: &mut egui::Ui,
    key: FilterKey,
    value: &str,
    enabled: bool,
    lang: Language,
) -> Option<AppAction> {
    let text = lang.requests();
    for (label, excluded) in [(text.filter_by_value, false), (text.exclude_value, true)] {
        let button = ui
            .add_enabled(enabled, egui::Button::new(label))
            .on_disabled_hover_text(text.tooltips.field_filter_needs_text);
        if button.clicked() {
            ui.close();
            return Some(AppAction::AppendFilter(field_term(key, value, excluded)));
        }
    }
    None
}

pub(crate) fn field_term(key: FilterKey, value: &str, excluded: bool) -> String {
    let prefix = if excluded { "-" } else { "" };
    let key = key.name();
    if value
        .chars()
        .any(|c| c.is_whitespace() || matches!(c, ',' | '"' | '\\'))
    {
        let escaped = value.replace('\\', "\\\\").replace('"', "\\\"");
        format!("{prefix}{key}:\"{escaped}\"")
    } else {
        format!("{prefix}{key}:{value}")
    }
}
