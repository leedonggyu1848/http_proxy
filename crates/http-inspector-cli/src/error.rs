//! Machine-readable error codes, their exit codes, and the error every
//! command reports.

use serde_json::{Value, json};

/// Why a command failed. The name is stable and documented in `docs/cli.md`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ErrorCode {
    /// The command line or a request is invalid.
    Usage,
    /// The `--filter` text has a term that cannot be used.
    InvalidFilter,
    /// No daemon answers on the control socket.
    DaemonNotRunning,
    /// The control directory or socket is not private to the current user.
    InsecureControlPath,
    /// The daemon did not become ready, or the control channel broke.
    DaemonUnavailable,
    /// Another daemon already listens on the control socket.
    DaemonAlreadyRunning,
    /// No retained record has the requested ID.
    RecordNotFound,
    /// The record has no body or response on the requested side.
    BodyUnavailable,
    /// The requested JSON path or `XPath` matches no tree row.
    PathNotFound,
    /// The body is binary and the requested view shows text.
    BinaryBody,
    /// The body format has no tree, or the requested view does not apply.
    ViewUnavailable,
    /// The proxy is not running.
    ProxyNotRunning,
    /// The proxy could not start or restart.
    ProxyStartFailed,
    /// `proxy start` asked for settings other than the running proxy's.
    ProxyConflict,
    /// HTTPS inspection could not be prepared.
    HttpsUnavailable,
    /// No record matched before the timeout.
    Timeout,
    /// A temporary condition; retrying shortly can succeed.
    Busy,
    /// The destination file already exists and is never overwritten.
    FileExists,
    /// A file or socket operation failed.
    Io,
}

impl ErrorCode {
    /// Every code, in documentation order.
    pub(crate) const ALL: [Self; 19] = [
        Self::Usage,
        Self::InvalidFilter,
        Self::DaemonNotRunning,
        Self::InsecureControlPath,
        Self::DaemonUnavailable,
        Self::DaemonAlreadyRunning,
        Self::RecordNotFound,
        Self::BodyUnavailable,
        Self::PathNotFound,
        Self::BinaryBody,
        Self::ViewUnavailable,
        Self::ProxyNotRunning,
        Self::ProxyStartFailed,
        Self::ProxyConflict,
        Self::HttpsUnavailable,
        Self::Timeout,
        Self::Busy,
        Self::FileExists,
        Self::Io,
    ];

    /// Stable snake-case name used in JSON output.
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::Usage => "usage",
            Self::InvalidFilter => "invalid_filter",
            Self::DaemonNotRunning => "daemon_not_running",
            Self::InsecureControlPath => "insecure_control_path",
            Self::DaemonUnavailable => "daemon_unavailable",
            Self::DaemonAlreadyRunning => "daemon_already_running",
            Self::RecordNotFound => "record_not_found",
            Self::BodyUnavailable => "body_unavailable",
            Self::PathNotFound => "path_not_found",
            Self::BinaryBody => "binary_body",
            Self::ViewUnavailable => "view_unavailable",
            Self::ProxyNotRunning => "proxy_not_running",
            Self::ProxyStartFailed => "proxy_start_failed",
            Self::ProxyConflict => "proxy_conflict",
            Self::HttpsUnavailable => "https_unavailable",
            Self::Timeout => "timeout",
            Self::Busy => "busy",
            Self::FileExists => "file_exists",
            Self::Io => "io",
        }
    }

    /// Process exit code for this error.
    pub(crate) const fn exit_code(self) -> u8 {
        match self {
            Self::Usage | Self::InvalidFilter => 2,
            Self::DaemonNotRunning => 3,
            Self::RecordNotFound | Self::BodyUnavailable | Self::PathNotFound => 4,
            Self::Timeout => 6,
            Self::Busy => 75,
            Self::InsecureControlPath
            | Self::DaemonUnavailable
            | Self::DaemonAlreadyRunning
            | Self::BinaryBody
            | Self::ViewUnavailable
            | Self::ProxyNotRunning
            | Self::ProxyStartFailed
            | Self::ProxyConflict
            | Self::HttpsUnavailable
            | Self::FileExists
            | Self::Io => 1,
        }
    }

    pub(crate) fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|code| code.name() == name)
    }
}

/// A failed command: a code to branch on, a message, and the next step.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
#[error("{message}")]
pub(crate) struct CliError {
    pub(crate) code: ErrorCode,
    pub(crate) message: String,
    pub(crate) hint: Option<String>,
}

impl CliError {
    pub(crate) fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            hint: None,
        }
    }

    #[must_use]
    pub(crate) fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    pub(crate) fn daemon_not_running() -> Self {
        Self::new(
            ErrorCode::DaemonNotRunning,
            "The HTTP Inspector daemon is not running",
        )
        .with_hint("run `http-proxy proxy start`")
    }

    pub(crate) fn io(context: &str, error: &std::io::Error) -> Self {
        Self::new(ErrorCode::Io, format!("{context}: {error}"))
    }

    /// The `error` object of JSON output and socket replies.
    pub(crate) fn to_json(&self) -> Value {
        json!({
            "code": self.code.name(),
            "message": self.message,
            "hint": self.hint,
        })
    }

    /// Reads an `error` object received from the daemon.
    pub(crate) fn from_json(value: &Value) -> Self {
        let code = value
            .get("code")
            .and_then(Value::as_str)
            .and_then(ErrorCode::from_name)
            .unwrap_or(ErrorCode::DaemonUnavailable);
        let message = value
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("The daemon sent an error without a message")
            .to_owned();
        Self {
            code,
            message,
            hint: value.get("hint").and_then(Value::as_str).map(str::to_owned),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_round_trip_and_exit_codes_are_documented_values() {
        for code in ErrorCode::ALL {
            assert_eq!(ErrorCode::from_name(code.name()), Some(code));
            assert!([1, 2, 3, 4, 6, 75].contains(&code.exit_code()), "{code:?}");
        }
        let error = CliError::daemon_not_running();
        assert_eq!(CliError::from_json(&error.to_json()), error);
    }
}
