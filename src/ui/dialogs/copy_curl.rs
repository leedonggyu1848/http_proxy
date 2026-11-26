//! "Copy as cURL" action, its redacted-header option, and the result notice.

use std::time::{Duration, Instant};

use eframe::egui;
use http_inspector_exchange::Exchange;
use http_inspector_interop::curl::{
    CURL_REDACTED_PLACEHOLDER, CurlUnavailable, RedactedHeaders, curl_command,
    curl_unavailable_reason,
};

use crate::ui::AppAction;
use crate::ui::i18n::{Language, Text, domain};
use crate::ui::style::indicator;
use crate::ui::style::tokens::Tone;

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
            Self::Copied => lang
                .pick("Copied as cURL.", "cURL로 복사했습니다.")
                .to_owned(),
            Self::CopiedWithPlaceholders(headers) => {
                let names = headers.join(", ");
                match lang {
                    Language::English => format!(
                        "Copied as cURL with {CURL_REDACTED_PLACEHOLDER} placeholders for redacted headers \
                         ({names}). Replace them with real values before running."
                    ),
                    Language::Korean => format!(
                        "가려진 헤더({names})에 {CURL_REDACTED_PLACEHOLDER} 자리표시자를 넣어 cURL로 복사했습니다. \
                         실행하기 전에 실제 값으로 바꾸세요."
                    ),
                }
            }
            Self::CopiedWithoutRedacted(headers) => {
                let names = headers.join(", ");
                match lang {
                    Language::English => format!(
                        "Copied as cURL without redacted headers ({names}). Their values are not stored; \
                         add them before running if the server needs them."
                    ),
                    Language::Korean => format!(
                        "가려진 헤더({names})를 제외하고 cURL로 복사했습니다. 원래 값은 저장되지 않으므로 \
                         서버에 필요하면 실행 전에 직접 추가하세요."
                    ),
                }
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
    /// Shows the copy button for `record` and the redacted-header option.
    pub(crate) fn controls(
        &self,
        ui: &mut egui::Ui,
        record: &Exchange,
        lang: Language,
        actions: &mut Vec<AppAction>,
    ) {
        if copy_button(ui, record, lang).clicked() {
            actions.push(AppAction::CopyCurl(record.id()));
        }
        let mut placeholders = self.redacted == RedactedHeaders::Placeholder;
        let hover = match lang {
            Language::English => format!(
                "Authorization and Cookie values are never stored. When on, the copied command \
                 carries them as {CURL_REDACTED_PLACEHOLDER} for you to fill in; when off, it \
                 leaves them out."
            ),
            Language::Korean => format!(
                "Authorization 및 Cookie 값은 저장되지 않습니다. 켜면 복사한 명령에 직접 채울 수 있도록 \
                 {CURL_REDACTED_PLACEHOLDER} 자리표시자를 넣고, 끄면 해당 헤더를 제외합니다."
            ),
        };
        if ui
            .checkbox(
                &mut placeholders,
                lang.pick("Redacted placeholders", "가려진 헤더 자리표시자"),
            )
            .on_hover_text(hover)
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
            ui.small_button(lang.text(Text::Dismiss)).clicked()
        })
        .inner
    }
}

/// Shows the copy button, disabled with the reason for records that have
/// no request to copy. Other problems are reported when it is pressed.
pub(crate) fn copy_button(ui: &mut egui::Ui, record: &Exchange, lang: Language) -> egui::Response {
    let unavailable = curl_unavailable_reason(record);
    let response = ui.add_enabled(
        unavailable.is_none(),
        egui::Button::new(lang.text(Text::CtxCopyAsCurl)),
    );
    match unavailable {
        Some(reason) => response.on_disabled_hover_text(domain::curl_unavailable(lang, &reason)),
        None => response.on_hover_text(lang.pick(
            "Copy this request as a curl command for bash or zsh.",
            "이 요청을 bash 또는 zsh용 curl 명령으로 복사합니다.",
        )),
    }
}

fn refused_text(reason: &CurlUnavailable, lang: Language) -> String {
    let detail = domain::curl_unavailable(lang, reason);
    match lang {
        Language::English => format!("Not copied as cURL: {detail}."),
        Language::Korean => format!("cURL로 복사하지 못했습니다: {detail}."),
    }
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
