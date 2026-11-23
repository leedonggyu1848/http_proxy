//! "Copy as cURL" action, its redacted-header option, and the result notice.

use std::time::{Duration, Instant};

use eframe::egui;
use http_proxy::Exchange;
use http_proxy::interop::curl::{
    CURL_REDACTED_PLACEHOLDER, CurlCommand, CurlUnavailable, RedactedHeaders, curl_command,
    curl_unavailable_reason,
};

use crate::ui::theme::Palette;

/// Label shared by the request-list menu and the details panel.
pub(in crate::ui) const COPY_LABEL: &str = "Copy as cURL";
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
pub(in crate::ui) struct CurlCopy {
    redacted: RedactedHeaders,
    notice: Option<Notice>,
}

impl CurlCopy {
    /// Shows the copy button for `record` and the redacted-header option.
    pub(in crate::ui) fn controls(&mut self, ui: &mut egui::Ui, record: &Exchange) {
        if copy_button(ui, record).clicked() {
            self.copy(ui.ctx(), record);
        }
        let mut placeholders = self.redacted == RedactedHeaders::Placeholder;
        if ui
            .checkbox(&mut placeholders, "Redacted placeholders")
            .on_hover_text(format!(
                "Authorization and Cookie values are never stored. When on, the copied command \
                 carries them as {CURL_REDACTED_PLACEHOLDER} for you to fill in; when off, it \
                 leaves them out."
            ))
            .changed()
        {
            self.redacted = if placeholders {
                RedactedHeaders::Placeholder
            } else {
                RedactedHeaders::Omit
            };
        }
    }

    /// Copies `record` as a `curl` command, or explains why it was not copied.
    pub(in crate::ui) fn copy(&mut self, context: &egui::Context, record: &Exchange) {
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

    /// Shows the latest result in the notice area.
    pub(in crate::ui) fn notice(&mut self, ui: &mut egui::Ui) {
        let Some(notice) = self.notice.as_ref() else {
            return;
        };
        if notice.kind == NoticeKind::Copied {
            let remaining = SUCCESS_NOTICE.saturating_sub(notice.shown_at.elapsed());
            if remaining.is_zero() {
                self.notice = None;
                return;
            }
            ui.ctx().request_repaint_after(remaining);
        }
        let palette = Palette::of(ui);
        let color = match notice.kind {
            NoticeKind::Copied => palette.success,
            NoticeKind::CopiedWithRedactions | NoticeKind::Refused => palette.warning,
        };
        let mut dismissed = false;
        ui.horizontal_wrapped(|ui| {
            ui.colored_label(color, &notice.text);
            dismissed = ui.small_button("Dismiss").clicked();
        });
        if dismissed {
            self.notice = None;
        }
    }
}

/// Shows the copy button, disabled with the reason for records that have
/// no request to copy. Other problems are reported when it is pressed.
pub(in crate::ui) fn copy_button(ui: &mut egui::Ui, record: &Exchange) -> egui::Response {
    let unavailable = curl_unavailable_reason(record);
    let response = ui.add_enabled(unavailable.is_none(), egui::Button::new(COPY_LABEL));
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
