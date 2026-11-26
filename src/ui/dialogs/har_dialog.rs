//! The `Export HAR` window: which records, what to include, and where to save.

use crate::ui::i18n::{Language, Text};
use crate::ui::style::tokens::Layout;
use std::path::PathBuf;

use eframe::egui;
use http_inspector_interop::har::HarOptions;

/// Which records an export contains.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum HarScope {
    /// Every record kept in the request list.
    #[default]
    All,
    /// Only the records that match the current filter and search.
    Filtered,
}

/// An export the user confirmed.
#[derive(Debug, Eq, PartialEq)]
pub(crate) struct HarExport {
    pub(crate) scope: HarScope,
    pub(crate) options: HarOptions,
    pub(crate) path: PathBuf,
}

pub(crate) enum HarAction {
    Close,
    Export(HarExport),
}

/// Numbers of records each scope would export.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ScopeCounts {
    pub(crate) all: usize,
    pub(crate) filtered: usize,
}

impl ScopeCounts {
    const fn of(self, scope: HarScope) -> usize {
        match scope {
            HarScope::All => self.all,
            HarScope::Filtered => self.filtered,
        }
    }
}

/// Window state. Options reset to the safe default each time it opens.
#[derive(Default)]
pub(crate) struct HarDialog {
    open: bool,
    path: String,
    scope: HarScope,
    options: HarOptions,
}

impl HarDialog {
    /// Opens the window with `path` as the suggested file and default options.
    pub(crate) fn open(&mut self, path: String) {
        *self = Self {
            open: true,
            path,
            ..Self::default()
        };
    }

    pub(crate) const fn is_open(&self) -> bool {
        self.open
    }

    pub(crate) const fn close(&mut self) {
        self.open = false;
    }

    /// Shows the form and reports what the user chose.
    pub(crate) fn show(
        &mut self,
        context: &egui::Context,
        counts: ScopeCounts,
        busy: bool,
        escape_enabled: bool,
        lang: Language,
    ) -> Option<HarAction> {
        if !self.open {
            return None;
        }
        let mut open = true;
        let mut action = None;
        let viewport = context.content_rect();
        let center = viewport.center();
        let max_width = Layout::HAR_WIDTH.fit(viewport.width());
        let max_height = Layout::HAR_HEIGHT.fit(viewport.height());
        egui::Window::new(lang.pick("Export HAR", "HAR 내보내기"))
            .id(egui::Id::unique("export-har"))
            .open(&mut open)
            .pivot(egui::Align2::CENTER_CENTER)
            .default_pos(center)
            .max_width(max_width)
            .max_height(max_height)
            .vscroll(true)
            .collapsible(false)
            .resizable(false)
            .show(context, |ui| action = self.contents(ui, counts, busy, lang));
        let escaped = escape_enabled && context.input(|input| input.key_pressed(egui::Key::Escape));
        action.or_else(|| (!open || escaped).then_some(HarAction::Close))
    }

    fn contents(
        &mut self,
        ui: &mut egui::Ui,
        counts: ScopeCounts,
        busy: bool,
        lang: Language,
    ) -> Option<HarAction> {
        ui.label(lang.pick("Records", "내보낼 기록"));
        let all_label = match lang {
            Language::English => format!("All requests in the list ({})", counts.all),
            Language::Korean => format!("목록의 모든 요청 ({}건)", counts.all),
        };
        let filtered_label = match lang {
            Language::English => format!("Requests matching the filter ({})", counts.filtered),
            Language::Korean => format!("현재 필터와 일치하는 요청 ({}건)", counts.filtered),
        };
        ui.radio_value(&mut self.scope, HarScope::All, all_label);
        ui.radio_value(&mut self.scope, HarScope::Filtered, filtered_label);
        ui.weak(lang.pick(
            "Only requests kept in the list since the last Clear are exported.",
            "마지막 비우기 이후 목록에 보관된 요청만 내보냅니다.",
        ));
        ui.separator();
        ui.label(lang.pick("Include (off by default)", "포함 항목 (기본 꺼짐)"));
        ui.checkbox(
            &mut self.options.include_bodies,
            lang.pick("Request and response bodies", "요청 및 응답 본문"),
        )
        .on_hover_text(lang.pick(
            "Includes in-memory previews (8 KiB per direction, 8 MiB for images; truncated bodies are noted in HAR comments and may contain secrets).",
            "메모리 미리보기(방향당 8 KiB, 이미지 최대 8 MiB)를 포함하며, 잘린 본문은 HAR 주석으로 표시되고 비밀번호·토큰이 포함될 수 있습니다.",
        ));
        ui.checkbox(
            &mut self.options.include_sensitive_headers,
            lang.pick(
                "Authorization and cookie header names",
                "Authorization 및 Cookie 헤더 이름",
            ),
        )
        .on_hover_text(lang.pick(
            "Adds Authorization, Proxy-Authorization, Cookie, and Set-Cookie as [REDACTED] without keeping original values.",
            "Authorization, Proxy-Authorization, Cookie, Set-Cookie 헤더를 원본 값 없이 [REDACTED]로 추가합니다.",
        ));
        ui.separator();
        ui.label(lang.pick("File", "파일"));
        let path_width = Layout::HAR_PATH_WIDTH.fit(ui.available_width());
        ui.add(
            egui::TextEdit::singleline(&mut self.path)
                .desired_width(path_width)
                .hint_text(lang.pick("Path of a new .har file", "새 .har 파일 경로")),
        );
        ui.weak(lang.pick(
            "An existing file is never replaced.",
            "이미 존재하는 파일은 덮어쓰지 않습니다.",
        ));
        let count = counts.of(self.scope);
        let ready = count > 0 && !self.path.trim().is_empty() && !busy;
        let export_label = match lang {
            Language::English => format!("Export {count} requests"),
            Language::Korean => format!("요청 {count}건 내보내기"),
        };
        ui.horizontal_wrapped(|ui| {
            let export = ui
                .add_enabled(ready, egui::Button::new(export_label))
                .clicked();
            let cancel = ui.button(lang.text(Text::Cancel)).clicked();
            if export {
                Some(HarAction::Export(HarExport {
                    scope: self.scope,
                    options: self.options,
                    path: PathBuf::from(self.path.trim()),
                }))
            } else {
                cancel.then_some(HarAction::Close)
            }
        })
        .inner
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escape_reports_close_without_changing_the_window_state() {
        let mut dialog = HarDialog::default();
        dialog.open("fixture.har".to_owned());
        let context = egui::Context::default();
        let input = egui::RawInput {
            events: vec![egui::Event::Key {
                key: egui::Key::Escape,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::default(),
            }],
            ..egui::RawInput::default()
        };
        let mut output = context.run_ui(input, |ui| {
            let action = dialog.show(
                ui.ctx(),
                ScopeCounts {
                    all: 1,
                    filtered: 1,
                },
                false,
                true,
                Language::English,
            );
            assert!(matches!(action, Some(HarAction::Close)));
            assert!(dialog.is_open());
        });
        output.textures_delta.clear();
        dialog.close();
        assert!(!dialog.is_open());
    }

    #[test]
    fn reopening_restores_the_safe_defaults() {
        let mut dialog = HarDialog::default();
        dialog.open("/tmp/first.har".into());
        dialog.options.include_bodies = true;
        dialog.options.include_sensitive_headers = true;
        dialog.scope = HarScope::Filtered;
        dialog.open("/tmp/second.har".into());
        assert!(dialog.open);
        assert_eq!(dialog.path, "/tmp/second.har");
        assert_eq!(dialog.scope, HarScope::All);
        assert_eq!(dialog.options, HarOptions::default());
    }
}
