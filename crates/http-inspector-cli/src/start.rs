//! `proxy start`: ask a running daemon, or start one in the background and
//! wait until it reports that the proxy listens.

use std::io::{BufRead as _, BufReader};
use std::os::unix::process::CommandExt as _;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use serde_json::Value;

use crate::args::{StartArgs, StartCommand};
use crate::client;
use crate::control::ControlPath;
use crate::daemon;
use crate::error::{CliError, ErrorCode};
use crate::output::{self, Kind, Output};
use crate::protocol::{Request, StartOptions, error_line, ok_line, read_reply};

/// Longest wait for a new daemon to report readiness. HTTPS inspection may
/// create the CA first.
const READY_TIMEOUT: Duration = Duration::from_secs(30);

pub(crate) fn start(
    control: &ControlPath,
    command: &StartCommand,
    output: Output,
) -> Result<(), CliError> {
    let options = StartOptions::from(&command.options);
    match client::call(control, &Request::Start(options)) {
        Ok(status) => {
            output.document(Kind::Status, &status);
            return Ok(());
        }
        Err(error) if error.code == ErrorCode::DaemonNotRunning => {}
        Err(error) => return Err(error),
    }
    let started = if command.foreground {
        daemon::run(control.clone(), options, |status| {
            output.document(Kind::Status, status);
        })
        .map(|()| None)
    } else {
        spawn(options).map(Some)
    };
    let status = match started {
        Ok(Some(status)) => status,
        Ok(None) => return Ok(()),
        // Another `proxy start` won the race; ask the daemon it started.
        Err(error) if error.code == ErrorCode::DaemonAlreadyRunning => {
            client::call(control, &Request::Start(options))?
        }
        Err(error) => return Err(error),
    };
    output.document(Kind::Status, &status);
    Ok(())
}

/// Starts `http-proxy __daemon` detached from this terminal and reads its
/// readiness line.
fn spawn(options: StartOptions) -> Result<Value, CliError> {
    let executable = std::env::current_exe()
        .map_err(|error| CliError::io("Could not find this program", &error))?;
    let mut command = Command::new(executable);
    command.arg("__daemon");
    if let Some(port) = options.port {
        command.arg("--port").arg(port.to_string());
    }
    if options.https_inspection {
        command.arg("--https-inspection");
    }
    let mut child = command
        .current_dir("/")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .map_err(|error| CliError::io("Could not start the daemon", &error))?;
    let stdout = child.stdout.take().ok_or_else(not_ready)?;
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let mut line = String::new();
        let read = BufReader::new(stdout).read_line(&mut line).map(|_| line);
        let _caller_gone = sender.send(read).is_err();
    });
    match receiver.recv_timeout(READY_TIMEOUT) {
        Ok(Ok(line)) if !line.trim().is_empty() => {
            let reply: Value = serde_json::from_str(&line).map_err(|_| not_ready())?;
            read_reply(&reply)
        }
        Ok(_) => {
            let _status = child.wait();
            Err(not_ready())
        }
        Err(_) => {
            let _killed = child.kill();
            Err(not_ready())
        }
    }
}

fn not_ready() -> CliError {
    CliError::new(
        ErrorCode::DaemonUnavailable,
        "The daemon exited before it reported that the proxy listens",
    )
    .with_hint("run `http-proxy proxy start --foreground` to see the error")
}

/// Runs inside the spawned daemon: serves until `proxy stop` and reports
/// readiness, or the startup error, as one line on stdout.
pub(crate) fn daemon_child(control: &ControlPath, args: &StartArgs) -> u8 {
    let result = daemon::run(control.clone(), StartOptions::from(args), |status| {
        output::ready_line(&ok_line(status));
    });
    match result {
        Ok(()) => 0,
        Err(error) => {
            output::ready_line(&error_line(&error));
            error.code.exit_code()
        }
    }
}
