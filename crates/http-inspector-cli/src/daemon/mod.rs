//! The headless daemon: it owns the proxy and the captured records and
//! answers requests on the control socket until `proxy stop`.

mod select;
mod session;

use std::io::{BufReader, Read as _, Write};
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::body;
use crate::control::ControlPath;
use crate::error::{CliError, ErrorCode};
use crate::protocol::{
    FilterSpec, MAX_REQUEST_BYTES, RecordRef, Request, StartOptions, error_line, ok_line,
};
use crate::view;
use select::{Selection, WatchState};
use session::{Session, finish_export};

/// How often queued exchanges are collected and waiters re-check records
/// whose state changes in place.
const TICK: Duration = Duration::from_millis(50);
/// Longest time `proxy stop` waits for open streams to say goodbye.
const STREAM_GOODBYE: Duration = Duration::from_millis(500);

struct Shared {
    session: Mutex<Session>,
    changed: Condvar,
    stopping: AtomicBool,
    streams: AtomicUsize,
    control: ControlPath,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, Session> {
        self.session.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Waits for the next tick or change, releasing the session meanwhile.
    fn wait<'a>(&self, guard: MutexGuard<'a, Session>, limit: Duration) -> MutexGuard<'a, Session> {
        self.changed
            .wait_timeout(guard, limit)
            .unwrap_or_else(PoisonError::into_inner)
            .0
    }

    fn status(&self, session: &Session) -> Value {
        let mut status = session.status();
        if let Some(status) = status.as_object_mut() {
            status.insert(
                "control_socket".to_owned(),
                json!(self.control.socket().to_string_lossy()),
            );
        }
        status
    }
}

/// Binds the control socket, starts the proxy, reports readiness through
/// `ready`, and serves requests until `proxy stop`.
pub(crate) fn run(
    control: ControlPath,
    options: StartOptions,
    ready: impl FnOnce(&Value),
) -> Result<(), CliError> {
    let bound = control.bind()?;
    let mut session = Session::new();
    if let Err(error) = session.start(options) {
        drop(bound.listener);
        control.remove_own_socket(bound.inode);
        return Err(error);
    }
    let shared = Arc::new(Shared {
        session: Mutex::new(session),
        changed: Condvar::new(),
        stopping: AtomicBool::new(false),
        streams: AtomicUsize::new(0),
        control,
    });
    let mut status = shared.status(&shared.lock());
    if let Some(status) = status.as_object_mut() {
        status.insert("already_running".to_owned(), json!(false));
    }
    ready(&status);
    let pump = Arc::clone(&shared);
    let pump = thread::Builder::new()
        .name("cli-pump".to_owned())
        .spawn(move || run_pump(&pump))
        .map_err(|error| CliError::io("Could not start the capture thread", &error))?;
    for stream in bound.listener.incoming() {
        if shared.stopping.load(Ordering::Acquire) {
            break;
        }
        let Ok(stream) = stream else { continue };
        let connection = Arc::clone(&shared);
        let _spawn_failed = thread::Builder::new()
            .name("cli-connection".to_owned())
            .spawn(move || serve(&connection, stream))
            .is_err();
    }
    // Refuse new clients before the socket file goes away.
    drop(bound.listener);
    say_goodbye(&shared);
    let _pump_panicked = pump.join().is_err();
    // `proxy stop` already stopped the proxy; this also covers a proxy that
    // is still running when the accept loop ends for another reason.
    let _stop_failed = shared.lock().stop_proxy().is_err();
    shared.control.remove_own_socket(bound.inode);
    Ok(())
}

/// Collects queued exchanges and wakes waiters on every tick.
#[expect(
    clippy::significant_drop_tightening,
    reason = "the condition variable releases the session while it waits"
)]
fn run_pump(shared: &Shared) {
    let mut session = shared.lock();
    while !shared.stopping.load(Ordering::Acquire) {
        let _arrived = session.drain();
        shared.changed.notify_all();
        session = shared.wait(session, TICK);
    }
}

/// Gives open `records watch` streams a moment to send their last event.
#[expect(
    clippy::significant_drop_tightening,
    reason = "the condition variable releases the session while it waits"
)]
fn say_goodbye(shared: &Shared) {
    shared.changed.notify_all();
    let deadline = Instant::now() + STREAM_GOODBYE;
    let mut session = shared.lock();
    while shared.streams.load(Ordering::Acquire) > 0 && Instant::now() < deadline {
        session = shared.wait(session, TICK);
    }
}

fn serve(shared: &Shared, stream: UnixStream) {
    let Ok(reader) = stream.try_clone() else {
        return;
    };
    let mut writer = stream;
    let request = read_request(BufReader::new(reader.take(MAX_REQUEST_BYTES)));
    let request = match request {
        Ok(request) => request,
        Err(error) => {
            let _gone = write_line(&mut writer, &error_line(&error)).is_err();
            return;
        }
    };
    if let Request::Watch { filter, since } = request {
        shared.streams.fetch_add(1, Ordering::AcqRel);
        let _gone = watch(shared, &filter, since, &mut writer).is_err();
        shared.streams.fetch_sub(1, Ordering::AcqRel);
        shared.changed.notify_all();
        return;
    }
    let stopping = matches!(request, Request::Stop);
    let reply = match respond(shared, request) {
        Ok(data) => ok_line(&data),
        Err(error) => error_line(&error),
    };
    let _gone = write_line(&mut writer, &reply).is_err();
    if stopping {
        // Wake the accept loop so it sees the stop flag.
        let _woken = UnixStream::connect(shared.control.socket()).is_ok();
    }
}

fn read_request(mut reader: impl std::io::BufRead) -> Result<Request, CliError> {
    let mut line = String::new();
    reader
        .read_line(&mut line)
        .map_err(|error| CliError::io("Could not read the request", &error))?;
    let value: Value = serde_json::from_str(&line)
        .map_err(|error| CliError::new(ErrorCode::Usage, format!("Malformed request: {error}")))?;
    Request::from_json(&value)
}

fn write_line(writer: &mut impl Write, value: &Value) -> std::io::Result<()> {
    let mut line = value.to_string();
    line.push('\n');
    writer.write_all(line.as_bytes())?;
    writer.flush()
}

fn respond(shared: &Shared, request: Request) -> Result<Value, CliError> {
    match request {
        Request::Status => Ok(shared.status(&shared.lock())),
        Request::Start(options) => {
            let mut session = shared.lock();
            let already_running = session.start(options)?;
            let mut status = shared.status(&session);
            drop(session);
            if let Some(status) = status.as_object_mut() {
                status.insert("already_running".to_owned(), json!(already_running));
            }
            Ok(status)
        }
        Request::Stop => {
            let mut session = shared.lock();
            session.stop_proxy()?;
            shared.stopping.store(true, Ordering::Release);
            shared.changed.notify_all();
            Ok(json!({ "stopped": true, "records": session.records().len() }))
        }
        Request::SetHttpsInspection(enabled) => {
            let mut session = shared.lock();
            session.set_https_inspection(enabled)?;
            let status = shared.status(&session);
            drop(session);
            Ok(status)
        }
        Request::List {
            filter,
            since,
            limit,
        } => list(shared, &filter, since, limit),
        Request::Show { id, part } => Ok(view::detail(shared.lock().find(id)?, part)),
        Request::Body(request) => body::render(shared.lock().find(request.id)?, &request),
        Request::Wait {
            filter,
            since,
            timeout,
        } => wait(shared, &filter, since, timeout),
        Request::Clear => Ok(json!({ "removed": shared.lock().clear() })),
        Request::SaveLog(path) => {
            let receiver = shared.lock().export_log(path)?;
            let saved = finish_export(&receiver)?;
            Ok(json!({ "path": saved.to_string_lossy() }))
        }
        Request::Watch { .. } => Err(CliError::new(
            ErrorCode::Usage,
            "records.watch is a streaming command",
        )),
    }
}

fn list(
    shared: &Shared,
    filter: &FilterSpec,
    since: Option<RecordRef>,
    limit: Option<usize>,
) -> Result<Value, CliError> {
    let mut selection = Selection::compile(filter)?;
    let session = shared.lock();
    let matched: Vec<Value> = session
        .records()
        .iter()
        .filter(|record| since.is_none_or(|since| since.precedes(record.id())))
        .filter(|record| selection.matches(record))
        .map(view::summary)
        .collect();
    let total = matched.len();
    let skip = limit.map_or(0, |limit| total.saturating_sub(limit));
    Ok(json!({
        "records": matched.into_iter().skip(skip).collect::<Vec<_>>(),
        "matched": total,
        "retained": session.records().len(),
    }))
}

fn wait(
    shared: &Shared,
    filter: &FilterSpec,
    since: Option<RecordRef>,
    timeout: Duration,
) -> Result<Value, CliError> {
    let mut selection = Selection::compile(filter)?;
    let deadline = Instant::now() + timeout;
    let mut session = shared.lock();
    loop {
        let found = session
            .records()
            .iter()
            .filter(|record| since.is_none_or(|since| since.precedes(record.id())))
            .find(|record| {
                http_inspector_analysis::search::is_settled(record) && selection.matches(record)
            });
        if let Some(record) = found {
            return Ok(json!({ "record": view::summary(record) }));
        }
        let now = Instant::now();
        if now >= deadline || shared.stopping.load(Ordering::Acquire) {
            return Err(CliError::new(
                ErrorCode::Timeout,
                format!(
                    "No completed record matched within {:.1} s",
                    timeout.as_secs_f64()
                ),
            )
            .with_hint("check the filter with `http-proxy records list --filter …`"));
        }
        session = shared.wait(session, deadline.saturating_duration_since(now).min(TICK));
    }
}

/// Streams record events as NDJSON until the client leaves or the daemon stops.
fn watch(
    shared: &Shared,
    filter: &FilterSpec,
    since: Option<RecordRef>,
    writer: &mut UnixStream,
) -> std::io::Result<()> {
    let probe = writer.try_clone()?;
    let selection = match Selection::compile(filter) {
        Ok(selection) => selection,
        Err(error) => return write_line(writer, &error_line(&error)),
    };
    let mut state = {
        let session = shared.lock();
        WatchState::new(selection, &session, since)
    };
    write_line(
        writer,
        &ok_line(&json!({ "event": "started", "since": state.since() })),
    )?;
    loop {
        let session = shared.wait(shared.lock(), TICK * 4);
        let events = state.events(&session);
        drop(session);
        let stopping = shared.stopping.load(Ordering::Acquire);
        for event in events {
            write_line(writer, &ok_line(&event))?;
        }
        if stopping {
            return write_line(writer, &ok_line(&json!({ "event": "daemon_stopped" })));
        }
        if client_left(&probe) {
            return Ok(());
        }
    }
}

/// Reports whether the client closed its end. Clients send nothing after
/// their request, so any read result other than "no data yet" means the
/// stream is over.
fn client_left(mut probe: &UnixStream) -> bool {
    if probe.set_nonblocking(true).is_err() {
        return true;
    }
    let mut byte = [0_u8; 1];
    let left = !matches!(
        probe.read(&mut byte),
        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock
    );
    let _restored = probe.set_nonblocking(false).is_ok();
    left
}
