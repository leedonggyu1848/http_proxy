use eframe::egui;
use http_inspector_analysis::filter::FilterKey;

use crate::ui::AppAction;
use crate::ui::workbench::text::ShellText;

pub(crate) fn items(
    ui: &mut egui::Ui,
    key: FilterKey,
    value: &str,
    enabled: bool,
) -> Option<AppAction> {
    for (label, excluded) in [
        (ShellText::FilterByValue.text(), false),
        (ShellText::ExcludeValue.text(), true),
    ] {
        let button = ui
            .add_enabled(enabled, egui::Button::new(label))
            .on_disabled_hover_text(ShellText::FieldFilterNeedsText.text());
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
