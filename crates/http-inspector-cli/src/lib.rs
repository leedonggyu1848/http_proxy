//! Command line interface for HTTP Inspector.
//!
//! `http-proxy <command>` talks to a headless daemon that owns the proxy and
//! the captured records. The daemon listens on a Unix domain socket that
//! only the current user can open; no TCP control port exists. Commands
//! print text for people or, with `--json`, versioned JSON documents for
//! scripts and agents. Display rules come from the same crates as the
//! desktop app, so both show the same bodies and select the same records.
//!
//! This crate has no `eframe`/`egui` dependency and needs no display.

#![forbid(unsafe_code)]

#[expect(
    clippy::redundant_pub_crate,
    reason = "crate-wide items of a private module; `pub` would trip `unreachable_pub`"
)]
mod args;
#[expect(
    clippy::redundant_pub_crate,
    reason = "crate-wide items of a private module; `pub` would trip `unreachable_pub`"
)]
mod body;
#[expect(
    clippy::redundant_pub_crate,
    reason = "crate-wide items of a private module; `pub` would trip `unreachable_pub`"
)]
mod client;
#[expect(
    clippy::redundant_pub_crate,
    reason = "crate-wide items of a private module; `pub` would trip `unreachable_pub`"
)]
mod control;
#[expect(
    clippy::redundant_pub_crate,
    reason = "crate-wide items of a private module; `pub` would trip `unreachable_pub`"
)]
mod daemon;
#[expect(
    clippy::redundant_pub_crate,
    reason = "crate-wide items of a private module; `pub` would trip `unreachable_pub`"
)]
mod error;
#[expect(
    clippy::redundant_pub_crate,
    reason = "crate-wide items of a private module; `pub` would trip `unreachable_pub`"
)]
mod output;
#[expect(
    clippy::redundant_pub_crate,
    reason = "crate-wide items of a private module; `pub` would trip `unreachable_pub`"
)]
mod protocol;
#[expect(
    clippy::redundant_pub_crate,
    reason = "crate-wide items of a private module; `pub` would trip `unreachable_pub`"
)]
mod schema;
#[expect(
    clippy::redundant_pub_crate,
    reason = "crate-wide items of a private module; `pub` would trip `unreachable_pub`"
)]
mod start;
#[expect(
    clippy::redundant_pub_crate,
    reason = "crate-wide items of a private module; `pub` would trip `unreachable_pub`"
)]
mod view;

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser as _;

use args::{Cli, Command, LogCommand, ProxyCommand, RecordsCommand};
use control::ControlPath;
use error::{CliError, ErrorCode};
use output::{Kind, Output};
use protocol::Request;

/// First arguments that select the command line instead of the desktop app.
const COMMANDS: [&str; 11] = [
    "proxy",
    "records",
    "log",
    "schema",
    "help",
    "__daemon",
    "--help",
    "-h",
    "--version",
    "-V",
    "--json",
];

/// Reports whether `args` (including the program name) run a command
/// instead of opening the desktop app. The app keeps its own `--port` option.
#[must_use]
pub fn is_cli_invocation(args: &[OsString]) -> bool {
    args.get(1)
        .and_then(|arg| arg.to_str())
        .is_some_and(|arg| COMMANDS.contains(&arg))
}

/// Runs one command and returns its exit code.
pub fn run(args: Vec<OsString>) -> ExitCode {
    let json = args.iter().any(|arg| arg == "--json");
    let cli = match Cli::try_parse_from(args) {
        Ok(cli) => cli,
        Err(error) => {
            let code = error.exit_code();
            if json && error.use_stderr() {
                Output::new(true).error(&CliError::new(ErrorCode::Usage, error.to_string()));
            } else {
                let _closed = error.print().is_err();
            }
            return ExitCode::from(u8::try_from(code).unwrap_or(2));
        }
    };
    let output = Output::new(cli.json);
    let control = ControlPath::locate(|name| std::env::var_os(name));
    match execute(&cli.command, &control, output) {
        Ok(code) => ExitCode::from(code),
        Err(error) => {
            output.error(&error);
            ExitCode::from(error.code.exit_code())
        }
    }
}

fn execute(command: &Command, control: &ControlPath, output: Output) -> Result<u8, CliError> {
    match command {
        Command::Proxy(ProxyCommand::Start(start)) => start::start(control, start, output)?,
        Command::Proxy(ProxyCommand::Stop) => {
            output.document(Kind::Stopped, &client::call(control, &Request::Stop)?);
        }
        Command::Proxy(ProxyCommand::Status) => {
            output.document(Kind::Status, &client::call(control, &Request::Status)?);
        }
        Command::Proxy(ProxyCommand::Set(set)) => {
            let enabled = matches!(set.https_inspection, args::OnOff::On);
            let status = client::call(control, &Request::SetHttpsInspection(enabled))?;
            output.document(Kind::Status, &status);
        }
        Command::Records(records @ RecordsCommand::Watch(_)) => {
            client::stream(control, &records.request(), |event| output.event(event))?;
        }
        Command::Records(records) => {
            let kind = match records {
                RecordsCommand::List(_) => Kind::List,
                RecordsCommand::Show(_) => Kind::Show,
                RecordsCommand::Body(_) => Kind::Body,
                RecordsCommand::Wait(_) => Kind::Wait,
                RecordsCommand::Watch(_) | RecordsCommand::Clear => Kind::Cleared,
            };
            output.document(kind, &client::call(control, &records.request())?);
        }
        Command::Log(LogCommand::Save { path }) => {
            let saved = client::call(control, &Request::SaveLog(absolute(path)?))?;
            output.document(Kind::LogSaved, &saved);
        }
        Command::Schema => output.document(Kind::Schema, &schema::document()),
        Command::Daemon(daemon) => return Ok(start::daemon_child(control, daemon)),
    }
    Ok(0)
}

/// Resolves `path` against this process's working directory, because the
/// daemon runs in `/`.
fn absolute(path: &std::path::Path) -> Result<PathBuf, CliError> {
    std::path::absolute(path).map_err(|error| {
        CliError::new(
            ErrorCode::Usage,
            format!("Could not resolve {}: {error}", path.display()),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_run_the_cli_and_other_arguments_open_the_app() {
        let args = |list: &[&str]| list.iter().map(OsString::from).collect::<Vec<_>>();
        assert!(is_cli_invocation(&args(&["http-proxy", "proxy", "start"])));
        assert!(is_cli_invocation(&args(&["http-proxy", "--help"])));
        assert!(!is_cli_invocation(&args(&["http-proxy"])));
        assert!(!is_cli_invocation(&args(&["http-proxy", "--port", "9000"])));
    }
}
