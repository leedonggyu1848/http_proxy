//! Sends one request to the daemon and reads its reply or event stream.

use std::io::{BufRead as _, BufReader, Write as _};
use std::os::unix::net::UnixStream;

use serde_json::Value;

use crate::control::ControlPath;
use crate::error::{CliError, ErrorCode};
use crate::protocol::{Request, read_reply};

/// Sends `request` and returns the data of its single reply.
pub(crate) fn call(control: &ControlPath, request: &Request) -> Result<Value, CliError> {
    let mut reader = send(control, request)?;
    next(&mut reader)?.ok_or_else(broken)?
}

/// Sends a streaming `request` and passes the data of each event to
/// `on_event` until the daemon ends the stream. An error reply ends it with
/// that error; `on_event` returning `false` ends it early.
pub(crate) fn stream(
    control: &ControlPath,
    request: &Request,
    mut on_event: impl FnMut(&Value) -> bool,
) -> Result<(), CliError> {
    let mut reader = send(control, request)?;
    while let Some(reply) = next(&mut reader)? {
        if !on_event(&reply?) {
            break;
        }
    }
    Ok(())
}

fn send(control: &ControlPath, request: &Request) -> Result<BufReader<UnixStream>, CliError> {
    let mut stream = control.connect()?;
    let mut line = request.to_json().to_string();
    line.push('\n');
    stream
        .write_all(line.as_bytes())
        .and_then(|()| stream.flush())
        .map_err(|error| CliError::io("Could not send the request to the daemon", &error))?;
    Ok(BufReader::new(stream))
}

/// Reads the next reply line, or `None` at the end of the stream.
fn next(reader: &mut BufReader<UnixStream>) -> Result<Option<Result<Value, CliError>>, CliError> {
    let mut line = String::new();
    let read = reader
        .read_line(&mut line)
        .map_err(|error| CliError::io("Could not read the daemon's reply", &error))?;
    if read == 0 {
        return Ok(None);
    }
    let value: Value = serde_json::from_str(&line).map_err(|_| broken())?;
    Ok(Some(read_reply(&value)))
}

fn broken() -> CliError {
    CliError::new(
        ErrorCode::DaemonUnavailable,
        "The daemon closed the connection without a reply",
    )
}
