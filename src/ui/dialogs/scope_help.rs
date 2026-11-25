//! Explains which traffic the proxy can and cannot record.

use eframe::egui;

/// One-line scope at the top of the capture help.
pub(super) const SCOPE_SUMMARY: &str = "Capture scope: only HTTP traffic that a client sends through this proxy. \
     Other traffic on this computer is not recorded.";

/// Explains what a CONNECT tunnel row does and does not contain.
pub(crate) const TUNNEL_NOTE: &str = "CONNECT tunnel forwarded without decryption, as HTTPS uses by default. \
     Only the host, port, byte counts, and duration are recorded. \
     URL paths, headers, and bodies inside the tunnel are not recorded. \
     Turn on decryption from the HTTPS button in the top bar to inspect requests in new connections.";

const RECORDED: [&str; 3] = [
    "HTTP requests and responses: method, URL, headers, and bodies.",
    "HTTPS with decryption off: one Tunnel row per CONNECT with only the host, port, byte counts, and duration.",
    "HTTPS with decryption on: each HTTP/1.1 request inside the connection, after the client trusts the app CA.",
];

const NOT_RECORDED: [&str; 4] = [
    "Programs that ignore proxy settings or open their own connections.",
    "Hosts excluded by NO_PROXY or a proxy bypass list, which often includes localhost.",
    "HTTP/3 (QUIC) over UDP. Browsers may switch to it after the first visit.",
    "Traffic sent through a VPN tunnel or another proxy instead of this one.",
];

const QUIC_HINT: &str = "Browsers opened with Open Browser already disable QUIC. \
     For a Chrome profile you start yourself, add --disable-quic so it uses HTTP/1.1 or HTTP/2 over TCP.";

fn disable_quic_example(address: std::net::SocketAddr) -> String {
    format!(
        "google-chrome --user-data-dir=/tmp/http-inspector-chrome --proxy-server=http://{address} --disable-quic"
    )
}

/// Shows the `?` button whose popup explains the capture scope.
pub(crate) fn button(ui: &mut egui::Ui, address: Option<std::net::SocketAddr>) {
    ui.menu_button("?", |ui| contents(ui, address))
        .response
        .on_hover_text("What is captured?");
}

fn contents(ui: &mut egui::Ui, address: Option<std::net::SocketAddr>) {
    let viewport = ui.ctx().content_rect();
    let max_width = (viewport.width() - 32.0).clamp(240.0, 520.0);
    let max_height = (viewport.height() - 80.0).max(160.0);
    ui.set_max_width(max_width);
    ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
        egui::ScrollArea::vertical()
            .max_height(max_height)
            .show(ui, |ui| {
                ui.label(SCOPE_SUMMARY);
                ui.add_space(4.0);
                ui.strong("Recorded");
                for line in RECORDED {
                    ui.label(format!("• {line}"));
                }
                ui.add_space(4.0);
                ui.strong("Not recorded");
                for line in NOT_RECORDED {
                    ui.colored_label(
                        crate::ui::style::theme::Palette::of(ui).warning,
                        format!("• {line}"),
                    );
                }
                ui.add_space(4.0);
                ui.label(QUIC_HINT);
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
        let gaps = NOT_RECORDED.join(" ");
        for expected in ["ignore proxy settings", "NO_PROXY", "QUIC", "VPN"] {
            assert!(gaps.contains(expected), "missing {expected}");
        }
        let example = disable_quic_example(std::net::SocketAddr::from(([127, 0, 0, 1], 18080)));
        assert!(example.contains("--disable-quic"));
        assert!(example.contains("http://127.0.0.1:18080"));
    }

    #[test]
    fn tunnel_note_says_contents_are_not_recorded() {
        assert!(TUNNEL_NOTE.contains("without decryption"));
        assert!(TUNNEL_NOTE.contains("bodies inside the tunnel are not recorded"));
    }

    #[test]
    fn popup_text_is_not_justified_inside_menu_layout() {
        let ctx = egui::Context::default();
        let address = std::net::SocketAddr::from(([127, 0, 0, 1], 18080));
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            ui.with_layout(egui::Layout::top_down_justified(egui::Align::Min), |ui| {
                contents(ui, Some(address));
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
