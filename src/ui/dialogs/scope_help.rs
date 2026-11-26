//! Explains which traffic the proxy can and cannot record.

use crate::ui::i18n::Language;
use crate::ui::style::tokens::{Layout, Space};
use eframe::egui;

/// Traffic the proxy records, as short labels (S1, D13). The explanation
/// is in `docs/guide/proxy-and-capture.md`.
const fn recorded(lang: Language) -> [&'static str; 3] {
    match lang {
        Language::English => [
            "HTTP: method, URL, headers, bodies",
            "HTTPS, Decrypt off: one Tunnel row per CONNECT",
            "HTTPS, Decrypt on: each HTTP/1.1 request",
        ],
        Language::Korean => [
            "HTTP: 메서드, URL, 헤더, 본문",
            "HTTPS(복호화 끔): CONNECT당 터널 행 1개",
            "HTTPS(복호화 켬): 각 HTTP/1.1 요청",
        ],
    }
}

/// Traffic the proxy cannot see, as short labels.
const fn not_recorded(lang: Language) -> [&'static str; 4] {
    match lang {
        Language::English => [
            "Apps that ignore proxy settings",
            "NO_PROXY and bypass-list hosts",
            "HTTP/3 (QUIC) over UDP",
            "Traffic through a VPN or another proxy",
        ],
        Language::Korean => [
            "프록시 설정을 무시하는 앱",
            "NO_PROXY 및 우회 목록 호스트",
            "UDP 기반 HTTP/3 (QUIC)",
            "VPN 또는 다른 프록시 경유 트래픽",
        ],
    }
}

/// Label of the Chrome command that keeps a self-started profile off QUIC.
const fn quic_command(lang: Language) -> &'static str {
    lang.pick("Chrome without QUIC", "QUIC 비활성화 Chrome 실행 명령")
}

fn disable_quic_example(address: std::net::SocketAddr) -> String {
    format!(
        "google-chrome --user-data-dir=/tmp/http-inspector-chrome --proxy-server=http://{address} --disable-quic"
    )
}

/// Shows the Capture scope submenu that explains which traffic is recorded.
pub(crate) fn submenu(ui: &mut egui::Ui, address: Option<std::net::SocketAddr>, lang: Language) {
    ui.menu_button(
        crate::ui::workbench::text::ShellText::CaptureScope.text(lang),
        |ui| contents(ui, address, lang),
    )
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
                let recorded_lines = recorded(lang);
                let not_recorded_lines = not_recorded(lang);
                for (title, lines) in [
                    (lang.pick("Recorded", "기록되는 항목"), &recorded_lines[..]),
                    (
                        lang.pick("Not recorded", "기록되지 않는 항목"),
                        &not_recorded_lines[..],
                    ),
                ] {
                    ui.strong(title);
                    for line in lines {
                        ui.label(*line);
                    }
                    ui.add_space(Space::XS);
                }
                if let Some(address) = address {
                    ui.strong(quic_command(lang));
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
    fn labels_are_not_sentences() {
        for lang in [Language::English, Language::Korean] {
            for line in recorded(lang).into_iter().chain(not_recorded(lang)) {
                assert!(!line.ends_with('.'), "{lang:?}: {line}");
            }
        }
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
