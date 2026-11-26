//! The daemon's capture session: the proxy, the retained records, and the
//! full-session log, managed like the desktop app's capture session.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use http_inspector_analysis::filter::RecordList;
use http_inspector_core::https::HttpsInspection;
use http_inspector_core::interception::{EngineState, ProxyConfig, ProxyHandle, StartError};
use http_inspector_core::recording::FullLog;
use http_inspector_core::replay::{ReplayDraft, ReplayError, ReplayTask};
use http_inspector_core::settings::{paths, port};
use http_inspector_exchange::Exchange;
use serde_json::{Value, json};

use crate::error::{CliError, ErrorCode};
use crate::protocol::{RecordRef, StartOptions};

/// Records retained for the list, as in the desktop app.
pub(crate) const MAX_RECORDS: usize = 2_000;
/// Longest wait for the session log to finish an export.
const EXPORT_TIMEOUT: Duration = Duration::from_secs(120);

pub(crate) struct Session {
    proxy: Option<ProxyHandle>,
    https_inspection: bool,
    /// The port the last proxy listened on, reused when it restarts.
    last_port: Option<u16>,
    proxy_error: Option<String>,
    records: RecordList<Exchange>,
    full_log: Result<FullLog, String>,
    /// Incremented by `records clear`.
    generation: u64,
}

impl Session {
    pub(crate) fn new() -> Self {
        Self {
            proxy: None,
            https_inspection: false,
            last_port: None,
            proxy_error: None,
            records: RecordList::new(MAX_RECORDS),
            full_log: FullLog::new().map_err(|error| error.to_string()),
            generation: 0,
        }
    }

    /// Starts the proxy unless it already runs. Returns `true` when it was
    /// already running.
    pub(crate) fn start(&mut self, options: StartOptions) -> Result<bool, CliError> {
        if let Some(proxy) = &self.proxy {
            self.check_running_matches(proxy.local_addr().port(), options)?;
            return Ok(true);
        }
        let port = options.port.unwrap_or_else(|| {
            let saved = paths::settings_directory(|name| std::env::var_os(name))
                .as_deref()
                .and_then(port::load_port);
            port::launch_port(std::iter::empty(), std::env::var_os(port::PORT_ENV), saved)
                .port
                .value()
        });
        self.launch(port, options.https_inspection)?;
        Ok(false)
    }

    /// A running proxy satisfies `proxy start` only with the same settings.
    /// A missing port or port 0 accepts any port; HTTPS inspection must
    /// match, because a script that asked for it must not proceed without it.
    fn check_running_matches(&self, port: u16, options: StartOptions) -> Result<(), CliError> {
        let port_differs = options
            .port
            .is_some_and(|wanted| wanted != 0 && wanted != port);
        let https_differs = options.https_inspection != self.https_inspection;
        if !port_differs && !https_differs {
            return Ok(());
        }
        let https = if self.https_inspection { "on" } else { "off" };
        let hint = if port_differs {
            "run `http-proxy proxy stop` first to use another port".to_owned()
        } else {
            format!(
                "run `http-proxy proxy set --https-inspection {}`",
                if options.https_inspection {
                    "on"
                } else {
                    "off"
                }
            )
        };
        Err(CliError::new(
            ErrorCode::ProxyConflict,
            format!("The running proxy listens on 127.0.0.1:{port} with HTTPS inspection {https}"),
        )
        .with_hint(hint))
    }

    fn launch(&mut self, port: u16, https_inspection: bool) -> Result<(), CliError> {
        let mut config = ProxyConfig::loopback(port);
        if https_inspection {
            let directory =
                paths::ca_directory(|name| std::env::var_os(name)).ok_or_else(|| {
                    CliError::new(
                        ErrorCode::HttpsUnavailable,
                        "No data directory for the HTTPS inspection CA",
                    )
                    .with_hint("set HOME or XDG_DATA_HOME")
                })?;
            config = config.with_https_inspection(HttpsInspection::new(directory));
        }
        let proxy = match &self.full_log {
            Ok(log) => ProxyHandle::start_recording(config, log.clone()),
            Err(_) => ProxyHandle::start(config),
        }
        .map_err(|error| start_error(port, &error))?;
        self.last_port = Some(proxy.local_addr().port());
        self.https_inspection = https_inspection;
        self.proxy_error = None;
        self.proxy = Some(proxy);
        Ok(())
    }

    /// Restarts the proxy on the same port with HTTPS inspection changed.
    pub(crate) fn set_https_inspection(&mut self, enabled: bool) -> Result<(), CliError> {
        if self.proxy.is_none() {
            return Err(
                CliError::new(ErrorCode::ProxyNotRunning, "The proxy is not running")
                    .with_hint("run `http-proxy proxy start`"),
            );
        }
        if enabled == self.https_inspection {
            return Ok(());
        }
        let port = self.last_port.unwrap_or_default();
        self.stop_proxy()?;
        self.launch(port, enabled)
    }

    /// Stops the proxy after taking the records it already queued.
    pub(crate) fn stop_proxy(&mut self) -> Result<(), CliError> {
        self.drain();
        let Some(proxy) = self.proxy.take() else {
            return Ok(());
        };
        self.records.proxy_stopped(proxy.dropped_records());
        proxy
            .stop()
            .map_err(|error| CliError::new(ErrorCode::Io, error.to_string()))
    }

    /// Moves queued exchanges into the record list. Returns whether any
    /// arrived. A proxy whose engine failed is released with its error.
    pub(crate) fn drain(&mut self) -> bool {
        let Some(proxy) = self.proxy.as_mut() else {
            return false;
        };
        let mut arrived = false;
        while let Some(record) = proxy.try_receive() {
            let _evicted = self.records.push(record);
            arrived = true;
        }
        if let EngineState::Failed { message } = proxy.state() {
            if let Some(proxy) = self.proxy.take() {
                self.records.proxy_stopped(proxy.dropped_records());
                let _stop_failed = proxy.stop().is_err();
            }
            self.proxy_error = Some(message);
            arrived = true;
        }
        arrived
    }

    /// The listening proxy address, or the last one while it is stopped.
    pub(crate) fn proxy_address(&self) -> SocketAddr {
        self.proxy.as_ref().map_or_else(
            || SocketAddr::from(([127, 0, 0, 1], self.last_port.unwrap_or_default())),
            ProxyHandle::local_addr,
        )
    }

    /// Sends a replay through the running proxy.
    pub(crate) fn replay(&self, draft: &ReplayDraft) -> Result<ReplayTask, ReplayError> {
        self.proxy
            .as_ref()
            .ok_or(ReplayError::ProxyNotRunning)?
            .replay(draft)
    }

    /// ID of the newest retained record, or 0.
    pub(crate) fn newest_id(&self) -> u64 {
        self.records
            .iter()
            .next_back()
            .map_or(0, |record| record.id().value())
    }

    pub(crate) const fn records(&self) -> &RecordList<Exchange> {
        &self.records
    }

    pub(crate) const fn generation(&self) -> u64 {
        self.generation
    }

    pub(crate) fn find(&self, id: RecordRef) -> Result<&Exchange, CliError> {
        self.records
            .iter()
            .find(|record| id.names(record.id()))
            .ok_or_else(|| {
                CliError::new(
                    ErrorCode::RecordNotFound,
                    format!("No retained record has ID {}", id.value()),
                )
                .with_hint("list record IDs with `http-proxy records list`")
            })
    }

    /// Removes every record, as **Clear** does. Returns how many were removed.
    pub(crate) fn clear(&mut self) -> usize {
        self.drain();
        let removed = self.records.len();
        let dropped = self.proxy.as_ref().map_or(0, ProxyHandle::dropped_records);
        self.records.clear(dropped);
        self.generation = self.generation.wrapping_add(1);
        removed
    }

    /// Hands the session log export to its writer thread. The caller waits
    /// for the result without holding the session lock.
    pub(crate) fn export_log(
        &self,
        path: PathBuf,
    ) -> Result<std::sync::mpsc::Receiver<std::io::Result<PathBuf>>, CliError> {
        let log = self.full_log.as_ref().map_err(|error| {
            CliError::new(
                ErrorCode::Io,
                format!("The session log is unavailable: {error}"),
            )
        })?;
        if path.exists() {
            return Err(file_exists(&path));
        }
        log.export(path).map_err(|error| export_error(&error))
    }

    pub(crate) fn status(&self) -> Value {
        let address = self.proxy.as_ref().map(ProxyHandle::local_addr);
        let engine_dropped = self.proxy.as_ref().map_or(0, ProxyHandle::dropped_records);
        let log = match &self.full_log {
            Ok(log) => json!({
                "available": true,
                "error": log.error(),
                "body_limit_bytes": log.body_limit(),
                "body_limit_reached": log.body_limit_reached(),
            }),
            Err(error) => json!({ "available": false, "error": error }),
        };
        json!({
            "running": self.proxy.is_some(),
            "address": address.map(|address| address.to_string()),
            "port": address.map(|address| address.port()),
            "https_inspection": self.proxy.is_some() && self.https_inspection,
            "proxy_error": self.proxy_error,
            "pid": std::process::id(),
            "records": self.records.len(),
            "record_limit": MAX_RECORDS,
            "records_dropped": self.records.dropped(engine_dropped),
            "records_evicted": self.records.evicted(),
            "session_log": log,
        })
    }
}

/// Waits for a session log export started by [`Session::export_log`].
pub(crate) fn finish_export(
    receiver: &std::sync::mpsc::Receiver<std::io::Result<PathBuf>>,
) -> Result<PathBuf, CliError> {
    match receiver.recv_timeout(EXPORT_TIMEOUT) {
        Ok(Ok(path)) => Ok(path),
        Ok(Err(error)) => Err(export_error(&error)),
        Err(_) => Err(CliError::new(
            ErrorCode::Busy,
            "The session log export did not finish in time",
        )
        .with_hint("retry `http-proxy log save` shortly")),
    }
}

fn file_exists(path: &std::path::Path) -> CliError {
    CliError::new(
        ErrorCode::FileExists,
        format!("{} already exists", path.display()),
    )
    .with_hint("choose a new file name; existing files are never overwritten")
}

fn export_error(error: &std::io::Error) -> CliError {
    match error.kind() {
        std::io::ErrorKind::WouldBlock => CliError::new(ErrorCode::Busy, error.to_string())
            .with_hint("retry `http-proxy log save` shortly"),
        std::io::ErrorKind::AlreadyExists => {
            CliError::new(ErrorCode::FileExists, error.to_string())
                .with_hint("choose a new file name; existing files are never overwritten")
        }
        _ => CliError::new(
            ErrorCode::Io,
            format!("Could not save the session log: {error}"),
        ),
    }
}

fn start_error(port: u16, error: &StartError) -> CliError {
    match error {
        StartError::Bind(_) => CliError::new(
            ErrorCode::ProxyStartFailed,
            format!("Could not listen on 127.0.0.1:{port}: {error}"),
        )
        .with_hint(
            "another program, the desktop app, or another daemon may use the port; try `--port 0`",
        ),
        StartError::Https(_) => CliError::new(ErrorCode::HttpsUnavailable, error.to_string()),
        _ => CliError::new(ErrorCode::ProxyStartFailed, error.to_string()),
    }
}
