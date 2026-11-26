//! "Copy as cURL" action, its redacted-header option, and the result notice.

use std::time::{Duration, Instant};

use eframe::egui;
use http_inspector_exchange::Exchange;
use http_inspector_interop::curl::{
    CURL_REDACTED_PLACEHOLDER, CurlCommand, CurlUnavailable, RedactedHeaders, curl_command,
    curl_unavailable_reason,
};

use crate::ui::AppAction;
use crate::ui::style::indicator;
use crate::ui::style::theme::Palette;
use crate::ui::style::tokens::Tone;

/// Label shared by the request-list menu and the details panel.
pub(crate) const COPY_LABEL: &str = "Copy as cURL";
/// How long a plain success notice stays before it hides itself.
const SUCCESS_NOTICE: Duration = Duration::from_secs(4);

/// What the last copy attempt did, shown until replaced, dismissed, or,
/// for a plain success, until it expires.
#[derive(Clone, Debug, Eq, PartialEq)]
struct Notice {
    text: String,
    kind: NoticeKind,
    shown_at: Instant,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum NoticeKind {
    /// Copied with every captured header.
    Copied,
    /// Copied, but redacted headers are missing or need values.
    CopiedWithRedactions,
    /// Not copied.
    Refused,
}

/// Copy option and the most recent result.
#[derive(Default)]
pub(crate) struct CurlCopy {
    redacted: RedactedHeaders,
    notice: Option<Notice>,
}

impl CurlCopy {
    /// The redaction choice for copied commands, in the Settings menu: it
    /// applies to every copy, not to the selected request.
    pub(crate) fn settings(&self, ui: &mut egui::Ui) -> Option<AppAction> {
        ui.label(egui::RichText::new(COPY_LABEL).color(Palette::of(ui).secondary));
        let mut placeholders = self.redacted == RedactedHeaders::Placeholder;
        let changed = ui
            .checkbox(&mut placeholders, "Redacted placeholders")
            .on_hover_text(format!(
                "Copied commands carry {CURL_REDACTED_PLACEHOLDER} for Authorization and Cookie values, which are never stored."
            ))
            .changed();
        let redacted = if placeholders {
            RedactedHeaders::Placeholder
        } else {
            RedactedHeaders::Omit
        };
        changed.then_some(AppAction::SetCurlRedactions(redacted))
    }

    /// Reports whether [`Self::notice`] has anything to show.
    pub(crate) const fn has_notice(&self) -> bool {
        self.notice.is_some()
    }

    /// Copies `record` as a `curl` command, or explains why it was not copied.
    pub(crate) fn copy(&mut self, context: &egui::Context, record: &Exchange) {
        let (text, kind) = match curl_command(record, self.redacted) {
            Ok(command) => {
                context.copy_text(command.text().to_owned());
                let text = copied_text(&command);
                let kind = if command.redacted_headers().is_empty() {
                    NoticeKind::Copied
                } else {
                    NoticeKind::CopiedWithRedactions
                };
                (text, kind)
            }
            Err(reason) => (refused_text(&reason), NoticeKind::Refused),
        };
        self.notice = Some(Notice {
            text,
            kind,
            shown_at: Instant::now(),
        });
    }

    pub(crate) const fn set_redactions(&mut self, redacted: RedactedHeaders) {
        self.redacted = redacted;
    }

    pub(crate) fn expire_notice(&mut self) {
        if self.notice.as_ref().is_some_and(|notice| {
            notice.kind == NoticeKind::Copied && notice.shown_at.elapsed() >= SUCCESS_NOTICE
        }) {
            self.dismiss_notice();
        }
    }

    pub(crate) fn dismiss_notice(&mut self) {
        self.notice = None;
    }

    /// Shows the latest result in the notice area and reports dismissal.
    pub(crate) fn notice(&self, ui: &mut egui::Ui) -> bool {
        let Some(notice) = self.notice.as_ref() else {
            return false;
        };
        if notice.kind == NoticeKind::Copied {
            ui.ctx()
                .request_repaint_after(SUCCESS_NOTICE.saturating_sub(notice.shown_at.elapsed()));
        }
        ui.horizontal_wrapped(|ui| {
            // A plain copy is the normal outcome; only the others need a mark.
            match notice.kind {
                NoticeKind::Copied => ui.label(&notice.text),
                NoticeKind::CopiedWithRedactions | NoticeKind::Refused => {
                    indicator::dot_label(ui, Tone::Warning, &notice.text)
                }
            };
            ui.small_button("Dismiss").clicked()
        })
        .inner
    }
}

/// Shows the copy button, disabled with the reason for records that have
/// no request to copy. Other problems are reported when it is pressed.
pub(crate) fn copy_button(ui: &mut egui::Ui, record: &Exchange) -> egui::Response {
    let unavailable = curl_unavailable_reason(record);
    let button = crate::ui::workbench::controls::ghost(
        COPY_LABEL,
        crate::ui::style::tokens::Layout::PANE_BUTTON_HEIGHT,
    );
    let response = ui.add_enabled(unavailable.is_none(), button);
    match unavailable {
        Some(reason) => response.on_disabled_hover_text(reason.to_string()),
        None => response.on_hover_text("Copy this request as a curl command for bash or zsh."),
    }
}

fn copied_text(command: &CurlCommand) -> String {
    let names = command.redacted_headers().join(", ");
    if names.is_empty() {
        "Copied as cURL.".to_owned()
    } else if command.has_placeholders() {
        format!(
            "Copied as cURL with {CURL_REDACTED_PLACEHOLDER} placeholders for redacted headers \
             ({names}). Replace them with real values before running."
        )
    } else {
        format!(
            "Copied as cURL without redacted headers ({names}). Their values are not stored; \
             add them before running if the server needs them."
        )
    }
}

fn refused_text(reason: &CurlUnavailable) -> String {
    format!("Not copied as cURL: {reason}.")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refusal_names_the_reason() {
        let text = refused_text(&CurlUnavailable::BodyTruncated {
            retained: 8192,
            observed: 10_000,
        });
        assert!(text.starts_with("Not copied as cURL: The request body was truncated"));
        assert!(text.contains("8192 of 10000 bytes"));
    }
}
