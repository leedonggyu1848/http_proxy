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
mod endpoints;
#[expect(
    clippy::redundant_pub_crate,
    reason = "crate-wide items of a private module; `pub` would trip `unreachable_pub`"
)]
mod error;
#[expect(
    clippy::redundant_pub_crate,
    reason = "crate-wide items of a private module; `pub` would trip `unreachable_pub`"
)]
mod local;
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

use args::{CaCommand, Cli, Command, LogCommand, ProxyCommand, RecordsCommand};
use control::ControlPath;
use error::{CliError, ErrorCode};
use output::{Kind, Output};
use protocol::Request;

/// First arguments that select the command line instead of the desktop app.
const COMMANDS: [&str; 12] = [
    "proxy",
    "records",
    "log",
    "ca",
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
        Command::Proxy(ProxyCommand::Env(env)) => {
            let status = client::call(control, &Request::Status)?;
            output.document(Kind::Env, &local::proxy_env(&status, env.ca_bundle)?);
        }
        Command::Ca(CaCommand::Info) => output.document(Kind::CaInfo, &local::ca_info()?),
        Command::Records(records) => run_records(records, control, output)?,
        Command::Log(LogCommand::Save { path }) => {
            let saved = client::call(control, &Request::SaveLog(absolute(path)?))?;
            output.document(Kind::LogSaved, &saved);
        }
        Command::Schema => output.document(Kind::Schema, &schema::document()),
        Command::Daemon(daemon) => return Ok(start::daemon_child(control, daemon)),
    }
    Ok(0)
}

/// Runs a `records` command. Paths are made absolute here, because the
/// daemon runs in `/`.
fn run_records(
    records: &RecordsCommand,
    control: &ControlPath,
    output: Output,
) -> Result<(), CliError> {
    let mut request = records.request();
    absolutize(&mut request)?;
    match (records, &request) {
        (RecordsCommand::Watch(_), _) => {
            client::stream(control, &request, |event| output.event(event))?;
        }
        (RecordsCommand::Hls(_), Request::Hls(hls)) if hls.output.is_some() => {
            client::stream(control, &request, |event| output.event(event))?;
        }
        (RecordsCommand::Export(_), Request::Export(export)) if export.output.is_none() => {
            let reply = client::call(control, &request)?;
            crate::output::raw(reply.get("har").unwrap_or(&serde_json::Value::Null));
        }
        (RecordsCommand::Endpoints(_), Request::Endpoints { openapi: true, .. }) => {
            let reply = client::call(control, &request)?;
            crate::output::raw(reply.get("openapi").unwrap_or(&serde_json::Value::Null));
        }
        _ => {
            let kind = match records {
                RecordsCommand::List(_) => Kind::List,
                RecordsCommand::Show(_) => Kind::Show,
                RecordsCommand::Body(_) => Kind::Body,
                RecordsCommand::Wait(_) => Kind::Wait,
                RecordsCommand::Curl(_) => Kind::Curl,
                RecordsCommand::Replay(_) => Kind::Replay,
                RecordsCommand::Export(_) => Kind::Exported,
                RecordsCommand::Endpoints(_) => Kind::Endpoints,
                RecordsCommand::Hls(_) => Kind::Hls,
                RecordsCommand::Watch(_) | RecordsCommand::Clear => Kind::Cleared,
            };
            output.document(kind, &client::call(control, &request)?);
        }
    }
    Ok(())
}

/// Makes the file paths of a request absolute.
fn absolutize(request: &mut Request) -> Result<(), CliError> {
    let path = match request {
        Request::Replay(replay) => &mut replay.body_file,
        Request::Export(export) => &mut export.output,
        Request::Hls(hls) => &mut hls.output,
        _ => return Ok(()),
    };
    if let Some(relative) = path.take() {
        *path = Some(absolute(&relative)?);
    }
    Ok(())
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
