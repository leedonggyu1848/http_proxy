//! Explains which traffic the proxy can and cannot record.

use crate::ui::i18n::{self, Language};
use crate::ui::style::tokens::{Layout, Space};
use eframe::egui;

/// Traffic the proxy records, as short labels (S1, D13). The explanation
/// is in `docs/guide/proxy-and-capture.md`.
const fn recorded(lang: Language) -> [&'static str; 3] {
    i18n::dialogs::scope_help::recorded_lines(lang)
}

/// Traffic the proxy cannot see, as short labels.
const fn not_recorded(lang: Language) -> [&'static str; 4] {
    i18n::dialogs::scope_help::not_recorded_lines(lang)
}

fn disable_quic_example(address: std::net::SocketAddr) -> String {
    format!(
        "google-chrome --user-data-dir=/tmp/http-inspector-chrome --proxy-server=http://{address} --disable-quic"
    )
}

/// Shows the Capture scope submenu that explains which traffic is recorded.
pub(crate) fn submenu(ui: &mut egui::Ui, address: Option<std::net::SocketAddr>, lang: Language) {
    let text = lang.scope_help();
    ui.menu_button(text.capture_scope, |ui| contents(ui, address, lang))
        .response
        .on_hover_text(text.tooltips.what_is_captured);
}

fn contents(ui: &mut egui::Ui, address: Option<std::net::SocketAddr>, lang: Language) {
    let text = lang.scope_help();
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
                    (text.recorded, &recorded_lines[..]),
                    (text.not_recorded, &not_recorded_lines[..]),
                ] {
                    ui.strong(title);
                    for line in lines {
                        ui.label(*line);
                    }
                    ui.add_space(Space::XS);
                }
                if let Some(address) = address {
                    ui.strong(text.quic_command);
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
