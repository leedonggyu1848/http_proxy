//! Explains which traffic the proxy can and cannot record.

use crate::ui::i18n::Language;
use crate::ui::style::tokens::{Layout, Space};
use eframe::egui;

/// One-line scope at the top of the capture help.
const fn scope_summary(lang: Language) -> &'static str {
    lang.pick(
        "Capture scope: only HTTP traffic that a client sends through this proxy. \
         Other traffic on this computer is not recorded.",
        "캡처 범위: 클라이언트가 이 프록시를 통해 보내는 HTTP 트래픽만 기록합니다. \
         이 컴퓨터의 다른 네트워크 트래픽은 기록되지 않습니다.",
    )
}

/// Explains what a CONNECT tunnel row does and does not contain.
pub(crate) const fn tunnel_note(lang: Language) -> &'static str {
    lang.pick(
        "CONNECT tunnel forwarded without decryption, as HTTPS uses by default. \
         Only the host, port, byte counts, and duration are recorded. \
         URL paths, headers, and bodies inside the tunnel are not recorded. \
         Enable Decrypt HTTPS to inspect requests in new connections.",
        "HTTPS 기본 동작에 따라 복호화 없이 전달된 CONNECT 터널입니다. \
         호스트, 포트, 전송 바이트 수, 소요 시간만 기록됩니다. \
         터널 내부의 URL 경로, 헤더, 본문은 기록되지 않습니다. \
         새 연결의 요청을 검사하려면 HTTPS 복호화를 켜세요.",
    )
}

const fn recorded(lang: Language) -> [&'static str; 3] {
    match lang {
        Language::English => [
            "HTTP requests and responses: method, URL, headers, and bodies.",
            "HTTPS with Decrypt HTTPS off: one Tunnel row per CONNECT with only the host, port, byte counts, and duration.",
            "HTTPS with Decrypt HTTPS on: each HTTP/1.1 request inside the connection, after the client trusts the app CA.",
        ],
        Language::Korean => [
            "HTTP 요청 및 응답: 메서드, URL, 헤더, 본문.",
            "HTTPS 복호화 꺼짐 상태의 HTTPS: CONNECT당 터널 행 1개(호스트, 포트, 바이트 수, 소요 시간만 기록).",
            "HTTPS 복호화 켜짐 상태의 HTTPS: 클라이언트가 앱 CA를 신뢰한 뒤 연결 내부의 각 HTTP/1.1 요청.",
        ],
    }
}

const fn not_recorded(lang: Language) -> [&'static str; 4] {
    match lang {
        Language::English => [
            "Programs that ignore proxy settings or open their own connections.",
            "Hosts excluded by NO_PROXY or a proxy bypass list, which often includes localhost.",
            "HTTP/3 (QUIC) over UDP. Browsers may switch to it after the first visit.",
            "Traffic sent through a VPN tunnel or another proxy instead of this one.",
        ],
        Language::Korean => [
            "프록시 설정을 무시하거나 직접 연결을 여는 프로그램.",
            "NO_PROXY 또는 프록시 우회 목록에 포함된 호스트(대개 localhost 포함).",
            "UDP 기반 HTTP/3 (QUIC). 브라우저는 첫 방문 이후 QUIC으로 전환할 수 있습니다.",
            "이 프록시 대신 VPN 터널이나 다른 프록시를 통해 전송되는 트래픽.",
        ],
    }
}

const fn quic_hint(lang: Language) -> &'static str {
    lang.pick(
        "Browsers opened with Open Browser already disable QUIC. \
         For a Chrome profile you start yourself, add --disable-quic so it uses HTTP/1.1 or HTTP/2 over TCP.",
        "'브라우저 열기'로 실행한 브라우저는 이미 QUIC이 비활성화되어 있습니다. \
         직접 실행하는 Chrome 프로필에서는 TCP 기반 HTTP/1.1 또는 HTTP/2를 사용하도록 --disable-quic 옵션을 추가하세요.",
    )
}

fn disable_quic_example(address: std::net::SocketAddr) -> String {
    format!(
        "google-chrome --user-data-dir=/tmp/http-inspector-chrome --proxy-server=http://{address} --disable-quic"
    )
}

/// Shows the `?` button whose popup explains the capture scope.
pub(crate) fn button(ui: &mut egui::Ui, address: Option<std::net::SocketAddr>, lang: Language) {
    ui.menu_button("?", |ui| contents(ui, address, lang))
        .response
        .on_hover_text(lang.pick("What is captured?", "무엇이 캡처되나요?"));
}

fn contents(ui: &mut egui::Ui, address: Option<std::net::SocketAddr>, lang: Language) {
    let viewport = ui.ctx().content_rect();
    let max_width = Layout::SCOPE_HELP_WIDTH.fit(viewport.width());
    let max_height = Layout::TOOLBAR_POPOVER_HEIGHT.fit(viewport.height());
    ui.set_max_width(max_width);
    ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
        egui::ScrollArea::vertical()
            .max_height(max_height)
            .show(ui, |ui| {
                ui.label(scope_summary(lang));
                ui.add_space(Space::XS);
                ui.strong(lang.pick("Recorded", "기록되는 항목"));
                for line in recorded(lang) {
                    ui.label(format!("• {line}"));
                }
                ui.add_space(Space::XS);
                ui.strong(lang.pick("Not recorded", "기록되지 않는 항목"));
                for line in not_recorded(lang) {
                    ui.colored_label(
                        crate::ui::style::theme::Palette::of(ui).warning,
                        format!("• {line}"),
                    );
                }
                ui.add_space(Space::XS);
                ui.label(quic_hint(lang));
                if let Some(address) = address {
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(disable_quic_example(address)).monospace(),
                        )
                        .wrap(),
                    );
                }
            });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn collect_text_shapes(shapes: &[egui::Shape], out: &mut Vec<(String, bool)>) {
        for shape in shapes {
            match shape {
                egui::Shape::Text(text) => {
                    out.push((text.galley.text().to_owned(), text.galley.job.justify));
                }
                egui::Shape::Vec(nested) => collect_text_shapes(nested, out),
                _ => {}
            }
        }
    }

    #[test]
    fn help_names_each_capture_gap() {
        let gaps = not_recorded(Language::English).join(" ");
        for expected in ["ignore proxy settings", "NO_PROXY", "QUIC", "VPN"] {
            assert!(gaps.contains(expected), "missing {expected}");
        }
        let ko_gaps = not_recorded(Language::Korean).join(" ");
        for expected in ["NO_PROXY", "QUIC", "VPN"] {
            assert!(ko_gaps.contains(expected), "missing {expected} in Korean");
        }
        let example = disable_quic_example(std::net::SocketAddr::from(([127, 0, 0, 1], 18080)));
        assert!(example.contains("--disable-quic"));
        assert!(example.contains("http://127.0.0.1:18080"));
    }

    #[test]
    fn tunnel_note_says_contents_are_not_recorded() {
        let en = tunnel_note(Language::English);
        assert!(en.contains("without decryption"));
        assert!(en.contains("bodies inside the tunnel are not recorded"));
        let ko = tunnel_note(Language::Korean);
        assert!(ko.contains("CONNECT"));
        assert!(ko.contains("기록되지 않습니다"));
    }

    #[test]
    fn popup_text_is_not_justified_inside_menu_layout() {
        let ctx = egui::Context::default();
        let address = std::net::SocketAddr::from(([127, 0, 0, 1], 18080));
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            ui.with_layout(egui::Layout::top_down_justified(egui::Align::Min), |ui| {
                contents(ui, Some(address), Language::English);
            });
        });
        output.textures_delta.clear();
        let shapes: Vec<_> = output
            .shapes
            .into_iter()
            .map(|clipped| clipped.shape)
            .collect();
        let mut texts = Vec::new();
        collect_text_shapes(&shapes, &mut texts);
        assert!(
            texts
                .iter()
                .any(|(text, _)| text.contains("--disable-quic")),
            "expected QUIC example command to render"
        );
        assert!(
            texts.iter().all(|(_, justified)| !justified),
            "expected no justified text in capture scope popup: {texts:?}"
        );
    }
}
