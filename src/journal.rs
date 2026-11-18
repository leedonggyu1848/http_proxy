use std::fs::File;
use std::io::{self, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::thread;

use tempfile::NamedTempFile;

const QUEUE_CAPACITY: usize = 1_024;
const EVENT_BYTES: usize = 8 * 1_024;
const BODY_CHUNK_BYTES: usize = 8 * 1_024;

type ExportResult = io::Result<PathBuf>;
type Failure = Arc<Mutex<Option<String>>>;

#[derive(Debug)]
enum Event {
    Append(Vec<u8>),
    Export(PathBuf, mpsc::Sender<ExportResult>),
}

/// A disk-backed session journal whose size is independent of UI retention.
#[derive(Clone, Debug)]
pub struct FullLog {
    sender: SyncSender<Event>,
    failure: Failure,
}

impl FullLog {
    /// Creates a private temporary journal and starts its writer.
    pub fn new() -> io::Result<Self> {
        let journal = NamedTempFile::new()?;
        let writer = BufWriter::new(journal.reopen()?);
        let (sender, receiver) = mpsc::sync_channel(QUEUE_CAPACITY);
        let failure = Arc::new(Mutex::new(None));
        let worker_failure = Arc::clone(&failure);
        thread::Builder::new()
            .name("session-journal".to_owned())
            .spawn(move || run(&journal, writer, &receiver, &worker_failure))?;
        Ok(Self { sender, failure })
    }

    /// Returns the first error that made this journal incomplete.
    #[must_use]
    pub fn error(&self) -> Option<String> {
        self.failure.lock().map_or_else(
            |_| Some("The session journal error state is unavailable".to_owned()),
            |failure| failure.clone(),
        )
    }

    pub(crate) fn text(&self, mut text: String) {
        if !text.ends_with('\n') {
            text.push('\n');
        }
        let Ok(mut failure) = self.failure.lock() else {
            return;
        };
        for chunk in text.as_bytes().chunks(EVENT_BYTES) {
            if self
                .submit_locked(Event::Append(chunk.to_vec()), &mut failure)
                .is_err()
            {
                break;
            }
        }
    }

    pub(crate) fn body(&self, id: u64, side: &str, bytes: &[u8]) {
        for chunk in bytes.chunks(BODY_CHUNK_BYTES) {
            self.text(format!(
                "[{id}] {side} body bytes: {}",
                chunk.escape_ascii()
            ));
        }
    }

    pub(crate) fn body_end(&self, id: u64, side: &str, state: &str) {
        self.text(format!("[{id}] {side} body: {state}"));
    }

    /// Saves all journal events accepted before this call to a new file.
    ///
    /// Existing files are never overwritten. The returned receiver reports
    /// completion without blocking the caller on disk I/O.
    pub fn export(&self, path: PathBuf) -> io::Result<Receiver<ExportResult>> {
        let (sender, receiver) = mpsc::channel();
        self.submit(Event::Export(path, sender))?;
        Ok(receiver)
    }

    fn submit(&self, event: Event) -> io::Result<()> {
        let mut failure = self
            .failure
            .lock()
            .map_err(|_| io::Error::other("The session journal error state is unavailable"))?;
        self.submit_locked(event, &mut failure)
    }

    fn submit_locked(&self, event: Event, failure: &mut Option<String>) -> io::Result<()> {
        if let Some(message) = failure.as_ref() {
            return Err(io::Error::other(message.clone()));
        }
        let is_export = matches!(&event, Event::Export(..));
        match self.sender.try_send(event) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(_)) if is_export => Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "Session logging is busy; retry saving the log shortly",
            )),
            Err(error) => {
                let message = match error {
                    TrySendError::Full(_) => {
                        "Session logging stopped because its queue filled; the log is incomplete"
                    }
                    TrySendError::Disconnected(_) => {
                        "Session logging stopped because its writer disconnected; the log is incomplete"
                    }
                }
                .to_owned();
                *failure = Some(message.clone());
                Err(io::Error::other(message))
            }
        }
    }
}

fn run(
    journal: &NamedTempFile,
    mut writer: BufWriter<File>,
    receiver: &Receiver<Event>,
    failure: &Failure,
) {
    let mut write_failure = None;
    for event in receiver {
        match event {
            Event::Append(bytes) => {
                if write_failure.is_none()
                    && let Err(error) = writer.write_all(&bytes)
                {
                    record_write_failure(failure, &mut write_failure, &error);
                }
            }
            Event::Export(path, result) => {
                if let Some(message) = write_failure.as_ref() {
                    let _receiver_closed = result.send(Err(io::Error::other(message.clone())));
                    continue;
                }
                if let Err(error) = writer.flush() {
                    record_write_failure(failure, &mut write_failure, &error);
                    let _receiver_closed = result.send(Err(error));
                    continue;
                }
                let snapshot = journal.reopen().and_then(|reader| {
                    let length = reader.metadata()?.len();
                    Ok((reader, length))
                });
                match snapshot {
                    Ok((reader, length)) => {
                        let spawn_failure = result.clone();
                        if let Err(error) = thread::Builder::new()
                            .name("session-export".to_owned())
                            .spawn(move || {
                                let _receiver_closed =
                                    result.send(copy_snapshot(reader, length, &path));
                            })
                        {
                            let _receiver_closed = spawn_failure.send(Err(error));
                        }
                    }
                    Err(error) => {
                        record_write_failure(failure, &mut write_failure, &error);
                        let _receiver_closed = result.send(Err(error));
                    }
                }
            }
        }
    }
}

fn record_write_failure(failure: &Failure, local: &mut Option<String>, error: &io::Error) {
    let message = format!("Session logging failed; the log is incomplete: {error}");
    *local = Some(message.clone());
    if let Ok(mut failure) = failure.lock() {
        failure.get_or_insert(message);
    }
}

fn copy_snapshot(reader: File, length: u64, path: &Path) -> ExportResult {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let mut destination = NamedTempFile::new_in(parent)?;
    let copied = io::copy(&mut reader.take(length), destination.as_file_mut())?;
    if copied != length {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "The session journal snapshot ended early",
        ));
    }
    destination.as_file_mut().sync_all()?;
    destination
        .persist_noclobber(path)
        .map_err(|error| error.error)?;
    Ok(path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use std::fmt::Write as _;

    use super::*;

    fn exported(log: &FullLog, path: PathBuf) -> io::Result<String> {
        log.export(path.clone())?
            .recv()
            .map_err(io::Error::other)??;
        std::fs::read_to_string(path)
    }

    #[test]
    fn retains_complete_large_bodies() -> io::Result<()> {
        let directory = tempfile::tempdir()?;
        let log = FullLog::new()?;
        let body: Vec<u8> = (0..=255).cycle().take(20_000).collect();
        log.body(7, "Request", &body);
        log.body_end(7, "Request", "complete");
        let text = exported(&log, directory.path().join("session.txt"))?;
        let mut expected = String::new();
        for chunk in body.chunks(BODY_CHUNK_BYTES) {
            writeln!(expected, "[7] Request body bytes: {}", chunk.escape_ascii())
                .map_err(io::Error::other)?;
        }
        assert_eq!(text, format!("{expected}[7] Request body: complete\n"));
        assert!(log.error().is_none());
        Ok(())
    }

    #[test]
    fn exports_have_independent_boundaries_and_do_not_overwrite() -> io::Result<()> {
        let directory = tempfile::tempdir()?;
        let log = FullLog::new()?;
        log.text("first".to_owned());
        let first_path = directory.path().join("first.txt");
        let first = log.export(first_path.clone())?;
        log.text("second".to_owned());
        let second_path = directory.path().join("second.txt");
        let second = log.export(second_path.clone())?;
        first.recv().map_err(io::Error::other)??;
        second.recv().map_err(io::Error::other)??;
        assert_eq!(std::fs::read_to_string(&first_path)?, "first\n");
        assert_eq!(std::fs::read_to_string(&second_path)?, "first\nsecond\n");
        assert!(
            log.export(first_path.clone())?
                .recv()
                .map_err(io::Error::other)?
                .is_err()
        );
        assert_eq!(std::fs::read_to_string(first_path)?, "first\n");
        assert!(log.error().is_none());
        Ok(())
    }

    #[test]
    fn queue_overflow_is_reported_and_blocks_export() {
        let (sender, _receiver) = mpsc::sync_channel(1);
        let log = FullLog {
            sender,
            failure: Arc::new(Mutex::new(None)),
        };
        log.text("first".to_owned());
        log.text("lost".to_owned());
        assert!(
            log.error()
                .is_some_and(|error| error.contains("queue filled"))
        );
        assert!(log.export(PathBuf::from("unused.txt")).is_err());
    }

    #[test]
    fn export_queue_overflow_is_retryable_without_invalidating_log() {
        let (sender, receiver) = mpsc::sync_channel(1);
        let log = FullLog {
            sender,
            failure: Arc::new(Mutex::new(None)),
        };
        log.text("first".to_owned());
        let error = log.export(PathBuf::from("unused.txt"));
        assert!(matches!(error, Err(error) if error.kind() == io::ErrorKind::WouldBlock));
        assert!(log.error().is_none());
        assert!(receiver.try_recv().is_ok());
        assert!(log.export(PathBuf::from("unused.txt")).is_ok());
    }

    #[test]
    fn later_queue_overflow_preserves_an_accepted_export() -> io::Result<()> {
        let directory = tempfile::tempdir()?;
        let journal = NamedTempFile::new()?;
        let writer = BufWriter::new(journal.reopen()?);
        let (sender, receiver) = mpsc::sync_channel(1);
        let failure = Arc::new(Mutex::new(None));
        let log = FullLog {
            sender,
            failure: Arc::clone(&failure),
        };
        let destination = directory.path().join("snapshot.txt");
        let saved = log.export(destination.clone())?;
        log.text("later event exceeds queue capacity".to_owned());
        assert!(log.error().is_some());
        drop(log);
        run(&journal, writer, &receiver, &failure);
        saved.recv().map_err(io::Error::other)??;
        assert_eq!(std::fs::read(destination)?, b"");
        Ok(())
    }

    #[test]
    fn journal_write_failure_is_reported_to_export() -> io::Result<()> {
        let directory = tempfile::tempdir()?;
        let journal = NamedTempFile::new()?;
        let writer = BufWriter::new(File::open(journal.path())?);
        let (sender, receiver) = mpsc::sync_channel(4);
        let failure = Arc::new(Mutex::new(None));
        let log = FullLog {
            sender,
            failure: Arc::clone(&failure),
        };
        log.text("cannot write to read-only journal".to_owned());
        let destination = directory.path().join("snapshot.txt");
        let saved = log.export(destination.clone())?;
        drop(log);
        run(&journal, writer, &receiver, &failure);
        assert!(saved.recv().map_err(io::Error::other)?.is_err());
        assert!(!destination.exists());
        let error = failure
            .lock()
            .map_err(|_| io::Error::other("poisoned error state"))?;
        assert!(
            error
                .as_ref()
                .is_some_and(|message| message.contains("incomplete"))
        );
        drop(error);
        Ok(())
    }
}
