//! Toolbar controls, launch port choice, and clipboard actions.

use crate::ui::app::capture::CaptureNotice;
#[cfg(test)]
use crate::ui::app::capture::PortChoice;
use crate::ui::app::state::SessionError;
use crate::ui::i18n::{Language, Text, domain};
use crate::ui::style::{
    indicator,
    tokens::{Layout, Tone},
};
use crate::ui::{AppAction, InspectorApp, MAX_RECORDS};
use eframe::egui;
use http_inspector_core::interception::{ProxyHandle, StartError};
use http_inspector_core::recording::FullLog;
use http_inspector_core::settings;
use http_inspector_exchange::ByteSize;
use std::borrow::Cow;

/// Explains a failed start in terms of what the user can change.
pub(crate) fn start_error_text(
    error: &StartError,
    port: settings::port::ProxyPort,
    lang: Language,
) -> String {
    domain::start_error(lang, error, port)
}

pub(crate) fn notice_text(notice: &CaptureNotice, lang: Language) -> Cow<'_, str> {
    match notice {
        CaptureNotice::InvalidPort(error) => Cow::Owned(domain::port_error(lang, error)),
        CaptureNotice::MissingCaDirectory => Cow::Borrowed(lang.pick(
            "HTTPS inspection requires an application data directory; check HOME and platform storage settings",
            "HTTPS 복호화를 사용하려면 앱 데이터 폴더가 필요합니다. HOME 및 플랫폼 저장소 설정을 확인하세요",
        )),
        CaptureNotice::Start { port, error } => Cow::Owned(start_error_text(error, *port, lang)),
        CaptureNotice::RememberPort(error) => Cow::Owned(domain::port_save_error(lang, error)),
        CaptureNotice::Stop(error) => Cow::Owned(domain::stop_error(lang, error)),
        CaptureNotice::Failed {
            message,
            stop_error,
        } => {
            let base = match lang {
                Language::English => {
                    format!("The proxy stopped: {message}. Press Start to restart it.")
                }
                Language::Korean => {
                    format!("프록시가 중지되었습니다: {message}. 다시 시작하려면 시작을 누르세요.")
                }
            };
            match stop_error {
                Some(error) => {
                    let stop_text = domain::stop_error(lang, error);
                    Cow::Owned(format!("{base} ({stop_text})"))
                }
                None => Cow::Owned(base),
            }
        }
    }
}

fn session_error_text(error: &SessionError, lang: Language) -> String {
    match error {
        SessionError::Replay(err) => domain::replay_error(lang, err),
        SessionError::RangeMerge(detail) => match lang {
            Language::English => format!("Merging ranges failed: {detail}"),
            Language::Korean => format!("범위 응답 합치기 실패: {detail}"),
        },
    }
}

const fn authorization_warning(lang: Language) -> &'static str {
    lang.pick(
        "Only enable inspection for traffic that you are authorized to inspect.",
        "HTTPS 복호화는 검사 권한이 있는 트래픽에만 켜세요.",
    )
}

/// Whether the one-time authorization warning for HTTPS inspection shows.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum AuthorizationNotice {
    #[default]
    NotShown,
    Showing,
    Dismissed,
}

impl AuthorizationNotice {
    /// Shows the warning the first time inspection is turned on.
    pub(crate) const fn inspection_enabled(self) -> Self {
        match self {
            Self::NotShown => Self::Showing,
            Self::Showing | Self::Dismissed => self,
        }
    }
}

impl InspectorApp {
    /// One toolbar row with the controls used while watching requests.
    pub(crate) fn toolbar(&self, ui: &mut egui::Ui, actions: &mut Vec<AppAction>) {
        let lang = self.language();
        ui.horizontal_wrapped(|ui| {
            self.proxy_controls(ui, actions);
            ui.separator();
            self.https_controls(ui, actions);
            ui.separator();
            actions.extend(
                self.dialogs
                    .browsers
                    .toolbar(
                        ui,
                        self.session.capture.proxy.is_some(),
                        self.session.capture.ca_directory.is_some()
                            && self.session.capture.https_inspection,
                        lang,
                    )
                    .map(AppAction::Browser),
            );
            actions.extend(
                self.dialogs
                    .save
                    .menu(ui, self.session.capture.full_log.is_some(), lang)
                    .into_iter()
                    .map(AppAction::SaveMenu),
            );
            ui.menu_button(lang.text(Text::Settings), |ui| {
                ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                    actions.extend(self.layout.theme.menu(ui, lang).map(AppAction::SetTheme));
                    ui.separator();
                    actions.extend(self.layout.language.menu(ui).map(AppAction::SetLanguage));
                    ui.separator();
                    actions.extend(
                        self.session
                            .body_saving
                            .settings_menu(ui, lang)
                            .map(AppAction::SetBodyStorageLimit),
                    );
                });
            });
            crate::ui::dialogs::scope_help::button(
                ui,
                self.session
                    .capture
                    .proxy
                    .as_ref()
                    .map(ProxyHandle::local_addr),
                lang,
            );
        });
    }

    fn proxy_controls(&self, ui: &mut egui::Ui, actions: &mut Vec<AppAction>) {
        let lang = self.language();
        if let Some(proxy) = self.session.capture.proxy.as_ref() {
            let address = proxy.local_addr();
            indicator::dot_label(ui, Tone::Success, lang.text(Text::StatusRunning));
            ui.monospace(address.to_string());
            actions.extend(copy_menu(ui, address, lang).map(AppAction::CopyText));
            if ui.button(lang.text(Text::Stop)).clicked() {
                actions.push(AppAction::StopProxy);
            }
        } else {
            indicator::dot_label(ui, Tone::Danger, lang.text(Text::StatusStopped));
            ui.label(lang.text(Text::PortLabel));
            let mut port = self.session.capture.port.text.clone();
            let edited = ui
                .add(egui::TextEdit::singleline(&mut port).desired_width(Layout::PORT_INPUT_WIDTH))
                .on_hover_text(lang.pick(
                    "Loopback port for the proxy (0 picks a free port; typed ports are remembered).",
                    "프록시가 사용할 로컬 포트입니다(0은 빈 포트 자동 선택, 입력한 포트는 저장).",
                ))
                .changed();
            if edited {
                actions.push(AppAction::SetPort(port));
            }
            if ui.button(lang.text(Text::Start)).clicked() {
                actions.push(AppAction::StartProxy);
            }
        }
    }

    fn https_controls(&self, ui: &mut egui::Ui, actions: &mut Vec<AppAction>) {
        let lang = self.language();
        let warning = authorization_warning(lang);
        let hover = if self.session.capture.proxy.is_some() {
            match lang {
                Language::English => format!("Stop the proxy to change this setting. {warning}"),
                Language::Korean => format!("이 설정을 바꾸려면 프록시를 중지하세요. {warning}"),
            }
        } else {
            match lang {
                Language::English => {
                    format!("Decrypt HTTPS in new connections with the app CA. {warning}")
                }
                Language::Korean => {
                    format!("앱 CA로 새 연결의 HTTPS를 복호화합니다. {warning}")
                }
            }
        };
        let mut enabled = self.session.capture.https_inspection;
        let toggled = ui
            .add_enabled(
                self.session.capture.proxy.is_none(),
                egui::Checkbox::new(&mut enabled, lang.text(Text::DecryptHttps)),
            )
            .on_hover_text(&hover)
            .on_disabled_hover_text(&hover)
            .changed();
        if toggled {
            actions.push(AppAction::SetHttpsInspection(enabled));
        }
        if self.session.capture.https_inspection {
            actions.extend(self.dialogs.ca_panel.summary(
                ui,
                self.session.capture.ca_directory.as_deref(),
                lang,
            ));
        }
    }

    /// Errors and warnings that need attention, shown right below the toolbar.
    pub(crate) fn notices(&self, ui: &mut egui::Ui, actions: &mut Vec<AppAction>) {
        let lang = self.language();
        if let Some(warning) = self.session.capture.port.launch_warning.as_ref() {
            indicator::dot_label(ui, Tone::Warning, warning);
        }
        if let Some(error) = self.session.capture.error.as_ref() {
            indicator::dot_label(ui, Tone::Danger, notice_text(error, lang));
        } else if let Some(error) = self.session.error.as_ref() {
            indicator::dot_label(ui, Tone::Danger, session_error_text(error, lang));
        }
        if let Some(error) = self.dialogs.browsers.error(lang) {
            indicator::dot_label(ui, Tone::Danger, error);
        }
        if self.detail.curl.notice(ui, lang) {
            actions.push(AppAction::DismissCurlNotice);
        }
        if self.session.capture.https_inspection {
            self.dialogs.ca_panel.messages(ui, lang);
            if self.dialogs.authorization_notice == AuthorizationNotice::Showing {
                ui.horizontal_wrapped(|ui| {
                    indicator::dot_label(ui, Tone::Warning, authorization_warning(lang));
                    if ui.small_button(lang.text(Text::Dismiss)).clicked() {
                        actions.push(AppAction::DismissAuthorization);
                    }
                });
            }
        }
    }

    /// Counters and full-session log state below the request list.
    pub(crate) fn status_bar(&self, ui: &mut egui::Ui, actions: &mut Vec<AppAction>) {
        let lang = self.language();
        ui.horizontal_wrapped(|ui| {
            let count = self.requests.records.len();
            ui.label(match lang {
                Language::English => format!("{count} requests"),
                Language::Korean => format!("요청 {count}건"),
            });
            let dropped = self
                .requests
                .records
                .dropped(self.session.capture.engine_dropped());
            if dropped > 0 {
                ui.separator();
                let label = match lang {
                    Language::English => format!("{dropped} records dropped"),
                    Language::Korean => format!("기록 {dropped}건 누락"),
                };
                indicator::dot_label(ui, Tone::Danger, label).on_hover_text(lang.pick(
                    "The capture queue was full, so these exchanges were forwarded but not shown.",
                    "캡처 대기열이 가득 차 이 요청들은 전달되었지만 목록에 표시되지 않았습니다.",
                ));
            }
            let evicted = self.requests.records.evicted();
            if evicted > 0 {
                ui.separator();
                let label = match lang {
                    Language::English => format!("{evicted} older records removed"),
                    Language::Korean => format!("오래된 기록 {evicted}건 정리됨"),
                };
                let hover = match lang {
                    Language::English => format!(
                        "Only the newest {MAX_RECORDS} requests stay on screen while the full-session log keeps every record."
                    ),
                    Language::Korean => format!(
                        "화면에는 최신 {MAX_RECORDS}건만 유지되며 전체 세션 로그에는 모든 기록이 남습니다."
                    ),
                };
                indicator::dot_label(ui, Tone::Warning, label).on_hover_text(hover);
            }
            if self.map.analysis.url_map().node_limit_reached() {
                ui.separator();
                let max_nodes = self.map.analysis.url_map().max_nodes();
                let message = match lang {
                    Language::English => format!(
                        "URL map reached its {max_nodes} node limit; deeper paths are folded into parent nodes."
                    ),
                    Language::Korean => format!(
                        "URL 맵이 최대 {max_nodes}개 노드 한도에 도달했습니다. 하위 경로는 상위 노드에 묶입니다."
                    ),
                };
                indicator::dot_label(ui, Tone::Warning, message);
            }
            self.full_log_status(ui, lang);
            self.dialogs.save.status(ui, lang);
            if self.session.body_saving.status(ui, lang) {
                actions.push(AppAction::CancelBulkSave);
            }
            self.layout.theme.status(ui, lang);
            self.layout.language.status(ui);
        });
    }

    fn full_log_status(&self, ui: &mut egui::Ui, lang: Language) {
        if let Some(error) = self
            .session
            .capture
            .log_init_error
            .as_ref()
            .map(|error| match lang {
                Language::English => format!("Full-session logging is unavailable: {error}"),
                Language::Korean => format!("전체 세션 로그를 사용할 수 없습니다: {error}"),
            })
            .or_else(|| {
                self.session
                    .capture
                    .full_log
                    .as_ref()
                    .and_then(FullLog::error)
            })
        {
            ui.separator();
            indicator::dot_label(ui, Tone::Danger, error);
        }
        if let Some(log) = self
            .session
            .capture
            .full_log
            .as_ref()
            .filter(|log| log.body_limit_reached())
        {
            ui.separator();
            let limit = ByteSize::new(log.body_limit()).exact();
            let message = match lang {
                Language::English => format!(
                    "The full-session log reached its {limit} body limit. Later body bytes are omitted; headers and results are still logged."
                ),
                Language::Korean => format!(
                    "전체 세션 로그가 본문 한도({limit})에 도달했습니다. 이후 본문은 생략되며 헤더와 결과만 계속 기록됩니다."
                ),
            };
            indicator::dot_label(ui, Tone::Warning, message);
        }
    }
}

/// Text placed on the clipboard for one of the toolbar copy actions.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum CopyTarget {
    Address,
    Curl,
    Environment,
}

impl CopyTarget {
    pub(super) const ALL: [Self; 3] = [Self::Address, Self::Curl, Self::Environment];

    pub(super) const fn label(self, lang: Language) -> &'static str {
        match self {
            Self::Address => lang.pick("Proxy URL", "프록시 URL"),
            Self::Curl => lang.pick("curl example", "curl 예시"),
            Self::Environment => lang.pick("HTTP_PROXY variables", "HTTP_PROXY 환경 변수"),
        }
    }

    pub(super) fn text(self, address: std::net::SocketAddr) -> String {
        let url = format!("http://{address}");
        match self {
            Self::Address => url,
            Self::Curl => format!("curl --proxy {url} http://example.com/"),
            Self::Environment => format!("HTTP_PROXY={url} HTTPS_PROXY={url}"),
        }
    }
}

fn copy_menu(ui: &mut egui::Ui, address: std::net::SocketAddr, lang: Language) -> Option<String> {
    let menu = ui.menu_button(lang.pick("Copy ▼", "복사 ▼"), |ui| {
        let mut copied = None;
        for target in CopyTarget::ALL {
            let text = target.text(address);
            if ui.button(target.label(lang)).on_hover_text(&text).clicked() {
                copied = Some(text);
                ui.close();
            }
        }
        copied
    });
    menu.inner.flatten()
}

#[cfg(test)]
mod port_choice_tests {
    use super::{PortChoice, start_error_text};
    use crate::ui::i18n::Language;
    use http_inspector_core::settings::port::{PortOrigin, PortSaveError, ProxyPort};
    use std::io;

    #[test]
    fn remember_reports_typed_errors_and_skips_non_user_ports()
    -> Result<(), Box<dyn std::error::Error>> {
        let port = ProxyPort::parse("8080").map_err(|error| error.to_string())?;
        let mut choice = PortChoice {
            text: port.to_string(),
            origin: PortOrigin::User,
            settings_directory: None,
            launch_warning: None,
        };
        assert!(matches!(
            choice.remember(port),
            Err(PortSaveError::SettingsDirectoryUnavailable)
        ));
        let file = tempfile::NamedTempFile::new()?;
        choice.settings_directory = Some(file.path().to_path_buf());
        assert!(matches!(choice.remember(port), Err(PortSaveError::Save(_))));
        choice.origin = PortOrigin::Environment;
        assert!(choice.remember(port).is_ok());
        Ok(())
    }

    #[test]
    fn explains_an_occupied_port() -> io::Result<()> {
        let occupied = std::net::TcpListener::bind("127.0.0.1:0")?;
        let port = ProxyPort::from(occupied.local_addr()?.port());
        let Err(error) = http_inspector_core::interception::ProxyHandle::start(
            http_inspector_core::interception::ProxyConfig::loopback(port.value()),
        ) else {
            return Err(io::Error::other("binding an occupied port succeeded"));
        };
        let text = start_error_text(&error, port, Language::English);
        assert!(text.contains("already in use"), "{text}");
        assert!(text.contains(&port.to_string()), "{text}");
        let ko = start_error_text(&error, port, Language::Korean);
        assert!(ko.contains("이미 사용 중"), "{ko}");
        assert!(ko.contains(&port.to_string()), "{ko}");
        Ok(())
    }
}

#[cfg(test)]
mod copy_tests {
    use super::CopyTarget;
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};
    #[test]
    fn formats_copy_texts_for_the_running_address() {
        let address = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 18080);
        assert_eq!(CopyTarget::Address.text(address), "http://127.0.0.1:18080");
        assert_eq!(
            CopyTarget::Curl.text(address),
            "curl --proxy http://127.0.0.1:18080 http://example.com/"
        );
        assert_eq!(
            CopyTarget::Environment.text(address),
            "HTTP_PROXY=http://127.0.0.1:18080 HTTPS_PROXY=http://127.0.0.1:18080"
        );
    }
}

#[cfg(test)]
mod authorization_tests {
    use super::AuthorizationNotice;

    #[test]
    fn warning_shows_only_the_first_time() {
        let shown = AuthorizationNotice::NotShown.inspection_enabled();
        assert_eq!(shown, AuthorizationNotice::Showing);
        assert_eq!(
            AuthorizationNotice::Dismissed.inspection_enabled(),
            AuthorizationNotice::Dismissed
        );
    }
}
