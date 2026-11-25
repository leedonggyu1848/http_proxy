//! "Copy as cURL" action, its redacted-header option, and the result notice.

use std::time::{Duration, Instant};

use eframe::egui;
use http_inspector_exchange::Exchange;
use http_inspector_interop::curl::{
    CURL_REDACTED_PLACEHOLDER, CurlCommand, CurlUnavailable, RedactedHeaders, curl_command,
    curl_unavailable_reason,
};

use crate::ui::AppAction;
use crate::ui::style::theme::Palette;

/// Label shared by the request-list menu and the details panel.
pub(super) const COPY_LABEL: &str = "Copy as cURL";
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
    /// Shows the copy button for `record` and the redacted-header option.
    pub(crate) fn controls(
        &self,
        ui: &mut egui::Ui,
        record: &Exchange,
        actions: &mut Vec<AppAction>,
    ) {
        if copy_button(ui, record).clicked() {
            actions.push(AppAction::CopyCurl(record.id()));
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
            let redacted = if placeholders {
                RedactedHeaders::Placeholder
            } else {
                RedactedHeaders::Omit
            };
            actions.push(AppAction::SetCurlRedactions(redacted));
        }
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
        let palette = Palette::of(ui);
        let color = match notice.kind {
            NoticeKind::Copied => palette.success,
            NoticeKind::CopiedWithRedactions | NoticeKind::Refused => palette.warning,
        };
        ui.horizontal_wrapped(|ui| {
            ui.colored_label(color, &notice.text);
            ui.small_button("Dismiss").clicked()
        })
        .inner
    }
}

/// Shows the copy button, disabled with the reason for records that have
/// no request to copy. Other problems are reported when it is pressed.
pub(crate) fn copy_button(ui: &mut egui::Ui, record: &Exchange) -> egui::Response {
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
