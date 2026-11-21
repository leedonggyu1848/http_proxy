//! Request filtering and case-insensitive search.

use crate::outcome_view::outcome_matches_query;
use http_proxy::{Exchange, HeaderPreview, Outcome};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RecordFilter {
    All,
    Responses,
    Failures,
    Tunnels,
}

impl RecordFilter {
    pub(crate) const ALL: [Self; 4] = [Self::All, Self::Responses, Self::Failures, Self::Tunnels];

    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Responses => "Responses",
            Self::Failures => "Failures",
            Self::Tunnels => "Tunnels",
        }
    }
}

pub(crate) fn record_matches(record: &Exchange, filter: RecordFilter, query: &str) -> bool {
    let outcome = record.outcome();
    let outcome_matches = match filter {
        RecordFilter::All => true,
        RecordFilter::Responses => matches!(outcome, Outcome::Response(_)),
        RecordFilter::Failures => outcome.is_failure(),
        RecordFilter::Tunnels => {
            matches!(outcome, Outcome::Tunnel(_) | Outcome::InspectionEstablished)
        }
    };
    let request = record.request();
    outcome_matches
        && (query.is_empty()
            || text_matches(request.method().as_str(), query)
            || text_matches(request.target(), query)
            || headers_match(request.headers(), query)
            || outcome_matches_query(outcome, query))
}

pub(crate) fn headers_match(headers: &HeaderPreview, query: &str) -> bool {
    headers
        .entries()
        .iter()
        .any(|header| text_matches(header.name(), query) || text_matches(header.value(), query))
}

pub(crate) fn text_matches(value: &str, lowercase_query: &str) -> bool {
    value.to_lowercase().contains(lowercase_query)
}

use crate::{InspectorApp, replay_panel};
use eframe::egui;

impl InspectorApp {
    pub(super) fn filters(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label("Filter");
            ui.add(
                egui::TextEdit::singleline(&mut self.filter_text)
                    .hint_text("method, URL, status, or header"),
            );
            egui::ComboBox::from_id_salt("record-filter")
                .selected_text(self.record_filter.label())
                .show_ui(ui, |ui| {
                    for filter in RecordFilter::ALL {
                        ui.selectable_value(&mut self.record_filter, filter, filter.label());
                    }
                });
        });
    }

    pub(super) fn request_table(&mut self, ui: &mut egui::Ui, shortcuts_allowed: bool) {
        let query = self.filter_text.to_lowercase();
        let rows: Vec<&Exchange> = self
            .records
            .iter()
            .filter(|record| record_matches(record, self.record_filter, &query))
            .collect();
        self.table.controls(ui);
        if let Some(original) = self.table.replay.take() {
            match replay_panel::ReplayPanel::new(original) {
                Ok(panel) => self.replay = Some(panel),
                Err(error) => self.error = Some(error),
            }
        }
        if let Some(selected) = self.table.show(ui, rows, self.selected, shortcuts_allowed) {
            self.selected = Some(selected);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use http_proxy::{ProxyConfig, ProxyHandle};
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    use tokio::net::{TcpListener, TcpStream};
    use tokio::time::{Duration, timeout};

    #[test]
    fn text_search_ignores_value_case() {
        assert!(text_matches("HeLLo WORLD", "hello world"));
        assert!(!text_matches("hello", "absent"));
    }

    #[tokio::test]
    async fn filters_and_search_captured_exchanges() -> Result<(), Box<dyn std::error::Error>> {
        timeout(Duration::from_secs(5), async {
            let upstream = TcpListener::bind("127.0.0.1:0").await?;
            let address = upstream.local_addr()?;
            let mut proxy = ProxyHandle::start(ProxyConfig::loopback(0))?;
            let mut client = TcpStream::connect(proxy.local_addr()).await?;
            client.write_all(format!("GET http://{address}/MiXeD HTTP/1.1\r\nHost: {address}\r\nX-Request: RequestVALUE\r\nConnection: close\r\n\r\n").as_bytes()).await?;
            let (mut server, _) = upstream.accept().await?;
            let pending = proxy.receive().await.ok_or("missing pending capture")?;
            check_filters(&pending, [true, false, false, false]);
            assert!(record_matches(&pending, RecordFilter::All, "pending"));
            assert!(record_matches(&pending, RecordFilter::All, "get"));
            assert!(record_matches(&pending, RecordFilter::All, "/mixed"));
            assert!(record_matches(&pending, RecordFilter::All, "x-request"));
            assert!(record_matches(&pending, RecordFilter::All, "requestvalue"));
            read_request_head(&mut server).await?;
            server.write_all(b"HTTP/1.1 201 Created\r\nContent-Length: 0\r\nX-Response: ResponseVALUE\r\nConnection: close\r\n\r\n").await?;
            let mut bytes = Vec::new();
            client.read_to_end(&mut bytes).await?;
            wait_settled(&pending).await;
            check_filters(&pending, [true, true, false, false]);
            for query in ["201", "x-response", "responsevalue"] {
                assert!(record_matches(&pending, RecordFilter::Responses, query), "{query}");
            }
            assert!(!record_matches(&pending, RecordFilter::All, "absent"));
            assert!(!record_matches(&pending, RecordFilter::Failures, "201"));

            let mut tunnel_client = TcpStream::connect(proxy.local_addr()).await?;
            tunnel_client.write_all(format!("CONNECT {address} HTTP/1.1\r\nHost: {address}\r\n\r\n").as_bytes()).await?;
            let (_tunnel_server, _) = upstream.accept().await?;
            let tunnel = proxy.receive().await.ok_or("missing tunnel capture")?;
            wait_settled(&tunnel).await;
            check_filters(&tunnel, [true, false, false, true]);
            assert!(record_matches(&tunnel, RecordFilter::Tunnels, "tunnel"));

            drop(upstream);
            let mut failed_client = TcpStream::connect(proxy.local_addr()).await?;
            failed_client.write_all(format!("GET http://{address}/ HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\n\r\n").as_bytes()).await?;
            let failed = proxy.receive().await.ok_or("missing failure capture")?;
            wait_settled(&failed).await;
            check_filters(&failed, [true, false, true, false]);
            assert!(record_matches(&failed, RecordFilter::Failures, "connect"));
            proxy.stop()?;
            Ok::<_, Box<dyn std::error::Error>>(())
        }).await??;
        Ok(())
    }

    fn check_filters(record: &Exchange, expected: [bool; 4]) {
        for (filter, expected) in RecordFilter::ALL.into_iter().zip(expected) {
            assert_eq!(
                record_matches(record, filter, ""),
                expected,
                "{filter:?}: {:?}",
                record.outcome()
            );
        }
    }

    /// Reads one request head so the reply cannot arrive before the proxy sends it.
    async fn read_request_head(stream: &mut TcpStream) -> std::io::Result<()> {
        let mut head = Vec::new();
        let mut byte = [0; 1];
        while !head.ends_with(b"\r\n\r\n") {
            stream.read_exact(&mut byte).await?;
            head.extend_from_slice(&byte);
        }
        Ok(())
    }

    async fn wait_settled(record: &Exchange) {
        while record.outcome().is_in_flight() {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    }
}
