//! Explains which traffic the proxy can and cannot record.

use eframe::egui;

/// One-line scope shown above the toolbar.
pub(super) const SCOPE_SUMMARY: &str = "Capture scope: only HTTP traffic that a client sends through this proxy. \
     Other traffic on this computer is not recorded.";

/// Explains what a CONNECT tunnel row does and does not contain.
pub(super) const TUNNEL_NOTE: &str = "CONNECT tunnel forwarded without decryption, as HTTPS uses by default. \
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

const DISABLE_QUIC_EXAMPLE: &str = "google-chrome --user-data-dir=/tmp/http-inspector-chrome \
     --proxy-server=http://127.0.0.1:8080 --disable-quic";

/// Shows the scope summary and the expandable capture help.
pub(super) fn show(ui: &mut egui::Ui) {
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
                ui.colored_label(crate::theme::Palette::of(ui).warning, format!("• {line}"));
            }
            ui.add_space(4.0);
            ui.label(QUIC_HINT);
            ui.monospace(DISABLE_QUIC_EXAMPLE);
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
        assert!(DISABLE_QUIC_EXAMPLE.contains("--disable-quic"));
    }

    #[test]
    fn tunnel_note_says_contents_are_not_recorded() {
        assert!(TUNNEL_NOTE.contains("without decryption"));
        assert!(TUNNEL_NOTE.contains("bodies inside the tunnel are not recorded"));
    }
}
