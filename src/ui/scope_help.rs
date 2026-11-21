//! Explains which traffic the proxy can and cannot record.

use eframe::egui::{self, Color32};

/// One-line scope shown above the toolbar.
pub(in crate::ui) const SCOPE_SUMMARY: &str = "Capture scope: only HTTP traffic that a client sends through this proxy. \
     Other traffic on this computer is not recorded.";

/// Explains what a CONNECT tunnel row does and does not contain.
pub(in crate::ui) const TUNNEL_NOTE: &str = "CONNECT tunnel forwarded without decryption, as HTTPS uses by default. \
     Only the host, port, byte counts, and duration are recorded. \
     URL paths, headers, and bodies inside the tunnel are not recorded. \
     Enable Decrypt HTTPS to inspect requests in new connections.";

const RECORDED: [&str; 3] = [
    "HTTP requests and responses: method, URL, headers, and bodies.",
    "HTTPS with Decrypt HTTPS off: one Tunnel row per CONNECT with only the host, port, byte counts, and duration.",
    "HTTPS with Decrypt HTTPS on: each HTTP/1.1 request inside the connection, after the client trusts the app CA.",
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

/// Shows the scope summary and the expandable capture help.
pub(in crate::ui) fn show(ui: &mut egui::Ui, address: Option<std::net::SocketAddr>) {
    ui.label(SCOPE_SUMMARY);
    egui::CollapsingHeader::new("What is captured?")
        .id_salt("capture-scope-help")
        .show(ui, |ui| {
            ui.strong("Recorded");
            for line in RECORDED {
                ui.label(format!("• {line}"));
            }
            ui.add_space(4.0);
            ui.strong("Not recorded");
            for line in NOT_RECORDED {
                ui.colored_label(Color32::YELLOW, format!("• {line}"));
            }
            ui.add_space(4.0);
            ui.label(QUIC_HINT);
            if let Some(address) = address {
                ui.monospace(disable_quic_example(address));
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
