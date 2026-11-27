//! "Copy as cURL" action, its redacted-header option, and the result notice.

use std::time::{Duration, Instant};

use eframe::egui;
use http_inspector_exchange::Exchange;
use http_inspector_interop::curl::{
    CurlUnavailable, RedactedHeaders, curl_command, curl_unavailable_reason,
};

use crate::ui::AppAction;
use crate::ui::i18n::{self, Language, domain};
use crate::ui::style::indicator;
use crate::ui::style::theme::Palette;
use crate::ui::style::tokens::Tone;

/// Label shared by the request-list menu and the details panel.
pub(crate) const fn copy_label(lang: Language) -> &'static str {
    lang.copy_curl().copy_as_curl
}

/// How long a plain success notice stays before it hides itself.
const SUCCESS_NOTICE: Duration = Duration::from_secs(4);

/// What the last copy attempt did, shown until replaced, dismissed, or,
/// for a plain success, until it expires.
#[derive(Clone, Debug, Eq, PartialEq)]
struct Notice {
    body: NoticeBody,
    shown_at: Instant,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum NoticeBody {
    /// Copied with every captured header.
    Copied,
    /// Copied with placeholder values for redacted headers.
    CopiedWithPlaceholders(Vec<String>),
    /// Copied omitting redacted headers.
    CopiedWithoutRedacted(Vec<String>),
    /// Not copied.
    Refused(CurlUnavailable),
}

impl NoticeBody {
    const fn is_plain_copy(&self) -> bool {
        matches!(self, Self::Copied)
    }

    fn text(&self, lang: Language) -> String {
        match self {
            Self::Copied => lang.copy_curl().copied.to_owned(),
            Self::CopiedWithPlaceholders(headers) => {
                i18n::dialogs::copy_curl::copied_with_placeholders(lang, &headers.join(", "))
            }
            Self::CopiedWithoutRedacted(headers) => {
                i18n::dialogs::copy_curl::copied_without_redacted(lang, &headers.join(", "))
            }
            Self::Refused(reason) => refused_text(reason, lang),
        }
    }
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
    pub(crate) fn settings(&self, ui: &mut egui::Ui, lang: Language) -> Option<AppAction> {
        ui.label(egui::RichText::new(copy_label(lang)).color(Palette::of(ui).secondary));
        let mut placeholders = self.redacted == RedactedHeaders::Placeholder;
        let hover = i18n::dialogs::copy_curl::redacted_placeholders_hint(lang);
        let changed = ui
            .checkbox(&mut placeholders, lang.copy_curl().redacted_placeholders)
            .on_hover_text(hover)
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
        let body = match curl_command(record, self.redacted) {
            Ok(command) => {
                context.copy_text(command.text().to_owned());
                let redacted: Vec<String> = command
                    .redacted_headers()
                    .iter()
                    .map(|name| (*name).clone())
                    .collect();
                if redacted.is_empty() {
                    NoticeBody::Copied
                } else if command.has_placeholders() {
                    NoticeBody::CopiedWithPlaceholders(redacted)
                } else {
                    NoticeBody::CopiedWithoutRedacted(redacted)
                }
            }
            Err(reason) => NoticeBody::Refused(reason),
        };
        self.notice = Some(Notice {
            body,
            shown_at: Instant::now(),
        });
    }

    pub(crate) const fn set_redactions(&mut self, redacted: RedactedHeaders) {
        self.redacted = redacted;
    }

    pub(crate) fn expire_notice(&mut self) {
        if self.notice.as_ref().is_some_and(|notice| {
            notice.body.is_plain_copy() && notice.shown_at.elapsed() >= SUCCESS_NOTICE
        }) {
            self.dismiss_notice();
        }
    }

    pub(crate) fn dismiss_notice(&mut self) {
        self.notice = None;
    }

    /// Shows the latest result in the notice area and reports dismissal.
    pub(crate) fn notice(&self, ui: &mut egui::Ui, lang: Language) -> bool {
        let Some(notice) = self.notice.as_ref() else {
            return false;
        };
        if notice.body.is_plain_copy() {
            ui.ctx()
                .request_repaint_after(SUCCESS_NOTICE.saturating_sub(notice.shown_at.elapsed()));
        }
        let text = notice.body.text(lang);
        ui.horizontal_wrapped(|ui| {
            // A plain copy is the normal outcome; only the others need a mark.
            if notice.body.is_plain_copy() {
                ui.label(text);
            } else {
                indicator::dot_label(ui, Tone::Warning, text);
            }
            ui.small_button(lang.shared().dismiss).clicked()
        })
        .inner
    }
}

/// Shows the copy button, disabled with the reason for records that have
/// no request to copy. Other problems are reported when it is pressed.
pub(crate) fn copy_button(ui: &mut egui::Ui, record: &Exchange, lang: Language) -> egui::Response {
    let unavailable = curl_unavailable_reason(record);
    let button = crate::ui::workbench::controls::ghost(
        copy_label(lang),
        crate::ui::style::tokens::Layout::PANE_BUTTON_HEIGHT,
    );
    let response = ui.add_enabled(unavailable.is_none(), button);
    match unavailable {
        Some(reason) => response.on_disabled_hover_text(domain::curl_unavailable(lang, &reason)),
        None => response.on_hover_text(lang.copy_curl().tooltips.copy_button_hint),
    }
}

fn refused_text(reason: &CurlUnavailable, lang: Language) -> String {
    let detail = domain::curl_unavailable(lang, reason);
    i18n::dialogs::copy_curl::refused(lang, &detail)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refusal_names_the_reason() {
        let reason = CurlUnavailable::BodyTruncated {
            retained: 8192,
            observed: 10_000,
        };
        let text = refused_text(&reason, Language::English);
        assert!(text.starts_with("Not copied as cURL: The request body was truncated"));
        assert!(text.contains("8192 of 10000 bytes"));

        let ko = refused_text(&reason, Language::Korean);
        assert!(ko.starts_with("cURL로 복사하지 못했습니다:"));
        assert!(ko.contains("8192"));
    }
}
