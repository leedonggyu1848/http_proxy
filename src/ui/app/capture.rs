//! The running proxy and exchange ingestion. Neither needs a display.

use std::io;
use std::path::PathBuf;

use http_inspector_analysis::filter::RecordList;
use http_inspector_analysis::pipeline::AnalysisPipeline;
use http_inspector_core::https::HttpsInspection;
use http_inspector_core::interception::{
    EngineState, ProxyConfig, ProxyHandle, StartError, StopError,
};
use http_inspector_core::recording::{BodyStore, FullLog};
use http_inspector_core::settings::{
    self,
    port::{PortError, PortSaveError, ProxyPort},
};
use http_inspector_exchange::{Exchange, RecordId};

/// Port typed in the toolbar and where its current value came from.
pub(crate) struct PortChoice {
    pub(crate) text: String,
    pub(crate) origin: settings::port::PortOrigin,
    pub(crate) settings_directory: Option<PathBuf>,
    pub(crate) launch_warning: Option<String>,
}

impl PortChoice {
    pub(crate) fn from_launch() -> Self {
        let settings_directory = settings::paths::settings_directory(|name| std::env::var_os(name));
        let saved = settings_directory
            .as_deref()
            .and_then(settings::port::load_port);
        let launch = settings::port::launch_port(
            std::env::args_os(),
            std::env::var_os(settings::port::PORT_ENV),
            saved,
        );
        Self {
            text: launch.port.to_string(),
            origin: launch.origin,
            settings_directory,
            launch_warning: launch.warning,
        }
    }

    /// Remembers a port the user typed once the proxy started on it.
    pub(crate) fn remember(
        &self,
        port: settings::port::ProxyPort,
    ) -> Result<(), settings::port::PortSaveError> {
        if self.origin != settings::port::PortOrigin::User {
            return Ok(());
        }
        let Some(directory) = self.settings_directory.as_deref() else {
            return Err(settings::port::PortSaveError::SettingsDirectoryUnavailable);
        };
        settings::port::save_port(directory, port).map_err(settings::port::PortSaveError::Save)
    }
}

pub(crate) enum CaptureNotice {
    InvalidPort(PortError),
    MissingCaDirectory,
    Start {
        port: ProxyPort,
        error: StartError,
    },
    RememberPort(PortSaveError),
    Stop(StopError),
    Failed {
        message: String,
        stop_error: Option<StopError>,
    },
}

pub(crate) struct CaptureSession {
    pub(crate) proxy: Option<ProxyHandle>,
    pub(crate) full_log: Option<FullLog>,
    pub(crate) log_init_error: Option<io::Error>,
    pub(crate) https_inspection: bool,
    pub(crate) ca_directory: Option<PathBuf>,
    pub(crate) port: PortChoice,
    pub(crate) error: Option<CaptureNotice>,
}

impl CaptureSession {
    pub(super) fn new() -> Self {
        let (full_log, log_init_error) = match FullLog::new() {
            Ok(log) => (Some(log), None),
            Err(error) => (None, Some(error)),
        };
        Self {
            proxy: None,
            full_log,
            log_init_error,
            https_inspection: false,
            ca_directory: settings::paths::ca_directory(|name| std::env::var_os(name)),
            port: PortChoice::from_launch(),
            error: None,
        }
    }

    pub(super) fn start(
        &mut self,
        store: Option<&BodyStore>,
        notify: impl Fn() + Send + Sync + 'static,
    ) {
        match self.start_handle(store, notify) {
            Ok(port) => {
                self.error = self
                    .port
                    .remember(port)
                    .err()
                    .map(CaptureNotice::RememberPort);
            }
            Err(error) => self.error = Some(error),
        }
    }

    fn start_handle(
        &mut self,
        store: Option<&BodyStore>,
        notify: impl Fn() + Send + Sync + 'static,
    ) -> Result<ProxyPort, CaptureNotice> {
        let port = ProxyPort::parse(&self.port.text).map_err(CaptureNotice::InvalidPort)?;
        let mut config = ProxyConfig::loopback(port.value());
        if self.https_inspection {
            let directory = self
                .ca_directory
                .clone()
                .ok_or(CaptureNotice::MissingCaDirectory)?;
            config = config.with_https_inspection(HttpsInspection::new(directory));
        }
        if let Some(store) = store {
            config = config.with_body_store(store.clone());
        }
        let proxy = match &self.full_log {
            Some(log) => ProxyHandle::start_recording(config, log.clone()),
            None => ProxyHandle::start(config),
        }
        .map_err(|error| CaptureNotice::Start { port, error })?;
        let _first_notifier = proxy.notify_on_event(notify);
        self.proxy = Some(proxy);
        Ok(port)
    }

    pub(crate) fn engine_dropped(&self) -> u64 {
        self.proxy.as_ref().map_or(0, ProxyHandle::dropped_records)
    }

    pub(super) fn stop(&mut self, records: &mut RecordList<Exchange>) {
        if let Some(proxy) = self.take_proxy(records)
            && let Err(error) = proxy.stop()
        {
            self.error = Some(CaptureNotice::Stop(error));
        }
    }

    fn take_proxy(&mut self, records: &mut RecordList<Exchange>) -> Option<ProxyHandle> {
        let proxy = self.proxy.take()?;
        records.proxy_stopped(proxy.dropped_records());
        Some(proxy)
    }

    pub(super) fn release_failed(
        &mut self,
        records: &mut RecordList<Exchange>,
        selected: &mut Option<RecordId>,
        analysis: &mut AnalysisPipeline,
    ) {
        if let Some(EngineState::Failed { message }) = self.proxy.as_ref().map(ProxyHandle::state) {
            self.release_after_failure(message, records, selected, analysis);
        }
    }

    fn release_after_failure(
        &mut self,
        message: String,
        records: &mut RecordList<Exchange>,
        selected: &mut Option<RecordId>,
        analysis: &mut AnalysisPipeline,
    ) {
        self.receive(records, selected, analysis);
        let stop_error = self
            .take_proxy(records)
            .and_then(|proxy| proxy.stop().err());
        self.error = Some(CaptureNotice::Failed {
            message,
            stop_error,
        });
    }

    pub(super) fn receive(
        &mut self,
        records: &mut RecordList<Exchange>,
        selected: &mut Option<RecordId>,
        analysis: &mut AnalysisPipeline,
    ) {
        let Some(proxy) = self.proxy.as_mut() else {
            return;
        };
        while let Some(record) = proxy.try_receive() {
            analysis.record_added(&record);
            if let Some(evicted) = add_record(records, selected, record) {
                analysis.record_evicted(&evicted);
            }
        }
    }

    #[cfg(test)]
    pub(super) fn fixture() -> Self {
        Self {
            proxy: None,
            full_log: None,
            log_init_error: None,
            https_inspection: false,
            ca_directory: None,
            port: PortChoice {
                text: "0".to_owned(),
                origin: settings::port::PortOrigin::Default,
                settings_directory: None,
                launch_warning: None,
            },
            error: None,
        }
    }
}

pub(super) fn add_record(
    records: &mut RecordList<Exchange>,
    selected: &mut Option<RecordId>,
    record: Exchange,
) -> Option<Exchange> {
    let evicted = records.push(record);
    if evicted
        .as_ref()
        .is_some_and(|record| *selected == Some(record.id()))
    {
        *selected = None;
    }
    evicted
}

#[cfg(test)]
mod tests {
    use std::error::Error;
    use std::net::{SocketAddr, TcpStream};
    use std::sync::mpsc;
    use std::time::Duration;

    use http::Method;
    use http_inspector_exchange::ExchangeBuilder;
    use tokio::io::{AsyncBufReadExt as _, AsyncReadExt as _, AsyncWriteExt as _, BufReader};
    use tokio::net::{TcpListener, TcpStream as AsyncStream};
    use tokio::time::timeout;

    use super::*;

    #[test]
    fn proxy_can_start_stop_and_restart() -> Result<(), Box<dyn Error>> {
        let mut capture = CaptureSession::fixture();
        let mut records = RecordList::new(2);
        capture.start(None, || {});
        assert!(capture.error.is_none());
        let address = capture
            .proxy
            .as_ref()
            .ok_or("proxy did not start")?
            .local_addr();
        assert!(TcpStream::connect(address).is_ok());

        capture.stop(&mut records);
        assert!(capture.error.is_none());
        assert!(capture.proxy.is_none());
        assert!(TcpStream::connect(address).is_err());

        capture.start(None, || {});
        assert!(capture.proxy.is_some());
        capture.stop(&mut records);
        Ok(())
    }

    #[tokio::test]
    async fn failure_drains_records_and_releases_the_proxy() -> Result<(), Box<dyn Error>> {
        let mut capture = CaptureSession::fixture();
        let mut records = RecordList::new(1);
        let mut analysis = AnalysisPipeline::disabled();
        let older = ExchangeBuilder::http(Method::GET, "http://fixture.test/older")?.in_flight()?;
        let mut selected = Some(older.id());
        let _evicted = add_record(&mut records, &mut selected, older);

        let (signal, notified) = mpsc::sync_channel(1);
        capture.start(None, move || {
            let _ = signal.try_send(());
        });
        let address = capture
            .proxy
            .as_ref()
            .ok_or("proxy did not start")?
            .local_addr();
        let target = capture_one_request(address).await?;
        notified.recv_timeout(Duration::from_secs(2))?;

        capture.release_after_failure(
            "test failure".to_owned(),
            &mut records,
            &mut selected,
            &mut analysis,
        );

        assert!(capture.proxy.is_none());
        assert!(
            matches!(capture.error, Some(CaptureNotice::Failed { message, stop_error: None }) if message == "test failure")
        );
        assert!(TcpStream::connect(address).is_err());
        assert_eq!(records.evicted(), 1);
        assert_eq!(selected, None);
        assert_eq!(
            records
                .iter()
                .next()
                .ok_or("record not received")?
                .request()
                .target(),
            target
        );
        assert!(analysis.url_map().node_count() > 0);
        Ok(())
    }

    async fn capture_one_request(proxy: SocketAddr) -> Result<String, Box<dyn Error>> {
        let upstream = TcpListener::bind("127.0.0.1:0").await?;
        let upstream_address = upstream.local_addr()?;
        let target = format!("http://{upstream_address}/new");
        let serve = async {
            let (connection, _) = upstream.accept().await?;
            let mut connection = BufReader::new(connection);
            let mut line = String::new();
            while connection.read_line(&mut line).await? != 0 {
                if line == "\r\n" {
                    break;
                }
                line.clear();
            }
            connection
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nOK")
                .await
        };
        let request = async {
            let mut connection = AsyncStream::connect(proxy).await?;
            let request = format!(
                "GET {target} HTTP/1.1\r\nHost: {upstream_address}\r\nConnection: close\r\n\r\n"
            );
            connection.write_all(request.as_bytes()).await?;
            let mut response = String::new();
            connection.read_to_string(&mut response).await?;
            Ok::<_, io::Error>(response)
        };
        let ((), response) = timeout(Duration::from_secs(2), async {
            tokio::try_join!(serve, request)
        })
        .await??;
        assert!(response.ends_with("OK"));
        Ok(target)
    }

    #[test]
    fn eviction_clears_only_the_selection_that_was_removed() -> Result<(), Box<dyn Error>> {
        let mut records = RecordList::new(2);
        let mut selected = None;
        let add = |records: &mut RecordList<Exchange>, selected: &mut Option<RecordId>, path| {
            let record = ExchangeBuilder::http(Method::GET, format!("http://fixture.test/{path}"))?
                .in_flight()?;
            let id = record.id();
            let _evicted = add_record(records, selected, record);
            Ok::<_, Box<dyn Error>>(id)
        };
        add(&mut records, &mut selected, "a")?;
        let id = add(&mut records, &mut selected, "b")?;
        selected = Some(id);
        add(&mut records, &mut selected, "c")?;
        assert_eq!(selected, Some(id));

        add(&mut records, &mut selected, "d")?;
        assert_eq!(selected, None);
        assert_eq!(records.evicted(), 2);
        Ok(())
    }
}
