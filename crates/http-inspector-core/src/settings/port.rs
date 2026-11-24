//! Choosing, validating, and remembering the proxy listener port.

use std::ffi::OsString;
use std::fmt;
use std::fs;
use std::io::{self, Write as _};
use std::path::Path;

use http_inspector_file::{ExistingFile, publish};

/// Port used when nothing else chooses one.
pub const DEFAULT_PORT: ProxyPort = ProxyPort(crate::DEFAULT_PORT);
/// Environment variable that overrides the saved port for one run.
pub const PORT_ENV: &str = "HTTP_INSPECTOR_PORT";
const SETTINGS_FILE: &str = "settings";
const PORT_KEY: &str = "proxy_port";

/// Validated loopback listener port. Zero asks the OS for a free port.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProxyPort(u16);

impl ProxyPort {
    /// Parses user input such as `8080` or `0`.
    pub fn parse(text: &str) -> Result<Self, PortError> {
        let text = text.trim();
        if text.is_empty() {
            return Err(PortError::Empty);
        }
        text.parse::<u16>()
            .map(Self)
            .map_err(|_| PortError::Invalid(text.to_owned()))
    }

    #[must_use]
    pub const fn value(self) -> u16 {
        self.0
    }
}

impl From<u16> for ProxyPort {
    /// Every `u16` is a valid listener port, so no validation is needed.
    fn from(value: u16) -> Self {
        Self(value)
    }
}

impl fmt::Display for ProxyPort {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

/// Reason a port value was rejected.
#[derive(Debug, Eq, PartialEq, thiserror::Error)]
pub enum PortError {
    #[error("Enter a port number, or 0 to pick a free port.")]
    Empty,
    #[error("\"{0}\" is not a port. Enter a number from 0 to 65535 (0 picks a free port).")]
    Invalid(String),
}

#[derive(Debug, Eq, PartialEq, thiserror::Error)]
enum PortArgumentError {
    #[error("--port ignored: it needs a value.")]
    MissingValue,
    #[error("Unknown argument \"{0}\" ignored. Use --port <number>.")]
    UnknownArgument(String),
    #[error("--port ignored: {0}")]
    InvalidPort(#[from] PortError),
}

/// Where the port of the current run came from.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PortOrigin {
    Default,
    Saved,
    Environment,
    CommandLine,
    /// Typed into the toolbar; only this origin is saved for the next launch.
    User,
}

/// Port chosen at launch, plus a problem to show when an override was invalid.
#[derive(Debug, Eq, PartialEq)]
pub struct LaunchPort {
    pub port: ProxyPort,
    pub origin: PortOrigin,
    pub warning: Option<String>,
}

/// Picks the launch port: `--port`, then the environment, then the saved
/// setting, then the default. An invalid override is reported and skipped.
pub fn launch_port(
    args: impl IntoIterator<Item = OsString>,
    env_value: Option<OsString>,
    saved: Option<ProxyPort>,
) -> LaunchPort {
    let mut warnings = Vec::new();
    let from_args = match port_argument(args) {
        Ok(port) => port,
        Err(error) => {
            warnings.push(error.to_string());
            None
        }
    };
    let from_env = env_value.and_then(|value| match ProxyPort::parse(&value.to_string_lossy()) {
        Ok(port) => Some(port),
        Err(error) => {
            warnings.push(format!("{PORT_ENV} ignored: {error}"));
            None
        }
    });
    let (port, origin) = from_args
        .map(|port| (port, PortOrigin::CommandLine))
        .or_else(|| from_env.map(|port| (port, PortOrigin::Environment)))
        .or_else(|| saved.map(|port| (port, PortOrigin::Saved)))
        .unwrap_or((DEFAULT_PORT, PortOrigin::Default));
    LaunchPort {
        port,
        origin,
        warning: (!warnings.is_empty()).then(|| warnings.join(" ")),
    }
}

/// Reads `--port N` or `--port=N`, skipping the program name.
fn port_argument(
    args: impl IntoIterator<Item = OsString>,
) -> Result<Option<ProxyPort>, PortArgumentError> {
    let mut args = args
        .into_iter()
        .skip(1)
        .map(|arg| arg.to_string_lossy().into_owned());
    let mut port = None;
    while let Some(arg) = args.next() {
        let value = if arg == "--port" {
            args.next().ok_or(PortArgumentError::MissingValue)?
        } else if let Some(value) = arg.strip_prefix("--port=") {
            value.to_owned()
        } else {
            return Err(PortArgumentError::UnknownArgument(arg));
        };
        port = Some(ProxyPort::parse(&value)?);
    }
    Ok(port)
}

/// Reads the saved port, treating a missing or malformed file as no setting.
#[must_use]
pub fn load_port(directory: &Path) -> Option<ProxyPort> {
    let text = fs::read_to_string(directory.join(SETTINGS_FILE)).ok()?;
    text.lines()
        .filter_map(|line| line.split_once('='))
        .find(|(key, _)| key.trim() == PORT_KEY)
        .and_then(|(_, value)| ProxyPort::parse(value).ok())
}

/// Reason a user-selected port could not be remembered.
#[derive(Debug, thiserror::Error)]
pub enum PortSaveError {
    #[error(
        "The port could not be saved because the application settings directory is unavailable."
    )]
    SettingsDirectoryUnavailable,
    #[error("The proxy started, but the port could not be saved: {0}")]
    Save(#[source] io::Error),
}

/// Saves `port` so the next launch starts with it.
pub fn save_port(directory: &Path, port: ProxyPort) -> io::Result<()> {
    fs::create_dir_all(directory)?;
    publish(
        &directory.join(SETTINGS_FILE),
        ExistingFile::Replace,
        |file| writeln!(file, "{PORT_KEY}={port}"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<OsString> {
        std::iter::once("http-proxy")
            .chain(values.iter().copied())
            .map(OsString::from)
            .collect()
    }

    #[test]
    fn parses_ports_and_rejects_other_text() {
        assert_eq!(ProxyPort::parse(" 9090 "), Ok(ProxyPort(9090)));
        assert_eq!(ProxyPort::parse("0"), Ok(ProxyPort(0)));
        assert_eq!(ProxyPort::parse(""), Err(PortError::Empty));
        assert_eq!(
            ProxyPort::parse("65536"),
            Err(PortError::Invalid("65536".to_owned()))
        );
        assert!(ProxyPort::parse("-1").is_err());
    }

    #[test]
    fn prefers_command_line_then_environment_then_saved() {
        let saved = Some(ProxyPort(7000));
        let launch = launch_port(args(&["--port", "9000"]), Some("8000".into()), saved);
        assert_eq!(
            (launch.port, launch.origin),
            (ProxyPort(9000), PortOrigin::CommandLine)
        );
        let launch = launch_port(args(&["--port=0"]), None, saved);
        assert_eq!(
            (launch.port, launch.origin),
            (ProxyPort(0), PortOrigin::CommandLine)
        );
        let launch = launch_port(args(&[]), Some("8000".into()), saved);
        assert_eq!(
            (launch.port, launch.origin),
            (ProxyPort(8000), PortOrigin::Environment)
        );
        let launch = launch_port(args(&[]), None, saved);
        assert_eq!(
            (launch.port, launch.origin),
            (ProxyPort(7000), PortOrigin::Saved)
        );
        let launch = launch_port(args(&[]), None, None);
        assert_eq!(
            (launch.port, launch.origin),
            (DEFAULT_PORT, PortOrigin::Default)
        );
        assert_eq!(launch.warning, None);
    }

    #[test]
    fn reports_and_skips_invalid_overrides() {
        let launch = launch_port(args(&["--port", "http"]), Some("x".into()), None);
        assert_eq!(launch.port, DEFAULT_PORT);
        let warning = launch.warning.unwrap_or_default();
        assert!(warning.contains("--port ignored"), "{warning}");
        assert!(warning.contains(PORT_ENV), "{warning}");

        let launch = launch_port(args(&["--verbose"]), None, None);
        assert!(
            launch
                .warning
                .is_some_and(|text| text.contains("--verbose"))
        );
        let launch = launch_port(args(&["--port"]), None, None);
        assert!(
            launch
                .warning
                .is_some_and(|text| text.contains("needs a value"))
        );
    }

    #[test]
    fn reads_settings_written_by_previous_versions() -> io::Result<()> {
        let directory = tempfile::tempdir()?;
        fs::write(directory.path().join("settings"), "proxy_port=9123\n")?;
        assert_eq!(load_port(directory.path()), Some(ProxyPort(9123)));
        fs::write(
            directory.path().join("settings"),
            "other=1\n proxy_port = 0 \n",
        )?;
        assert_eq!(load_port(directory.path()), Some(ProxyPort(0)));
        save_port(directory.path(), ProxyPort(18081))?;
        assert_eq!(
            fs::read_to_string(directory.path().join("settings"))?,
            "proxy_port=18081\n"
        );
        Ok(())
    }

    #[test]
    fn saves_and_loads_the_port() -> io::Result<()> {
        let directory = tempfile::tempdir()?;
        let settings = directory.path().join("nested");
        assert_eq!(load_port(&settings), None);
        save_port(&settings, ProxyPort(18080))?;
        assert_eq!(load_port(&settings), Some(ProxyPort(18080)));
        save_port(&settings, ProxyPort(0))?;
        assert_eq!(load_port(&settings), Some(ProxyPort(0)));
        fs::write(settings.join(SETTINGS_FILE), "proxy_port=nope\n")?;
        assert_eq!(load_port(&settings), None);
        Ok(())
    }
}
