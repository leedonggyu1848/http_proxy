//! Saving many stored bodies into one folder with a `manifest.json`.

use std::fs::{DirBuilder, OpenOptions};
use std::io::{self, Write as _};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use tempfile::NamedTempFile;

use super::file_name::url_directories;
use super::{BodyStore, SaveBody, SaveFormat, SavedBody};
use crate::engine::journal::BodySide;

const MANIFEST: &str = "manifest.json";

/// How saved files are arranged inside the output folder.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum BodyLayout {
    /// Every file directly in the output folder.
    #[default]
    Flat,
    /// Files in `host/path/` folders that follow each request URL.
    HostPath,
}

/// Progress of a running [`BulkSave`], shared with its worker.
#[derive(Debug)]
pub struct BulkProgress {
    total: usize,
    done: AtomicUsize,
    bytes: AtomicU64,
    cancelled: AtomicBool,
}

impl BulkProgress {
    /// Returns how many bodies the save covers.
    #[must_use]
    pub const fn total(&self) -> usize {
        self.total
    }

    /// Returns how many bodies were handled, saved or failed.
    #[must_use]
    pub fn done(&self) -> usize {
        self.done.load(Ordering::Acquire)
    }

    /// Returns the bytes written so far.
    #[must_use]
    pub fn bytes(&self) -> u64 {
        self.bytes.load(Ordering::Acquire)
    }

    /// Reports whether cancellation was requested.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }
}

/// What a finished bulk save wrote.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BulkSummary {
    directory: PathBuf,
    saved: usize,
    incomplete: usize,
    failed: usize,
    cancelled: bool,
}

impl BulkSummary {
    /// Returns the output folder, which also holds `manifest.json`.
    #[must_use]
    pub fn directory(&self) -> &Path {
        &self.directory
    }

    /// Returns how many files were written, including incomplete ones.
    #[must_use]
    pub const fn saved(&self) -> usize {
        self.saved
    }

    /// Returns how many written files hold an incomplete body.
    #[must_use]
    pub const fn incomplete(&self) -> usize {
        self.incomplete
    }

    /// Returns how many bodies could not be written.
    #[must_use]
    pub const fn failed(&self) -> usize {
        self.failed
    }

    /// Reports whether the save stopped early because it was cancelled.
    #[must_use]
    pub const fn cancelled(&self) -> bool {
        self.cancelled
    }
}

/// A bulk save running on its own thread.
#[derive(Debug)]
pub struct BulkSave {
    progress: Arc<BulkProgress>,
    result: Receiver<io::Result<BulkSummary>>,
}

impl BulkSave {
    /// Returns the live progress.
    #[must_use]
    pub fn progress(&self) -> &BulkProgress {
        &self.progress
    }

    /// Asks the save to stop after the body it is writing. Files already
    /// written stay and are listed in the manifest.
    pub fn cancel(&self) {
        self.progress.cancelled.store(true, Ordering::Release);
    }

    /// Returns the result once the save finished, without waiting.
    #[must_use]
    pub fn try_finish(&self) -> Option<io::Result<BulkSummary>> {
        match self.result.try_recv() {
            Ok(result) => Some(result),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => Some(Err(io::Error::other(
                "The bulk save worker stopped unexpectedly",
            ))),
        }
    }
}

impl BodyStore {
    /// Saves `bodies` into the new folder `directory` in the background and
    /// writes `manifest.json` there listing every body, saved or not.
    ///
    /// The folder must not exist yet; it is created readable only by this
    /// user. The directory each body names is ignored.
    pub fn save_all(
        &self,
        bodies: Vec<SaveBody>,
        layout: BodyLayout,
        directory: PathBuf,
    ) -> io::Result<BulkSave> {
        DirBuilder::new().mode(0o700).create(&directory)?;
        // Claim the manifest name first so a body named `manifest.json` gets
        // a numbered name instead.
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(directory.join(MANIFEST))?;
        let progress = Arc::new(BulkProgress {
            total: bodies.len(),
            done: AtomicUsize::new(0),
            bytes: AtomicU64::new(0),
            cancelled: AtomicBool::new(false),
        });
        let (sender, result) = mpsc::channel();
        let store = self.clone();
        let worker_progress = Arc::clone(&progress);
        thread::Builder::new()
            .name("body-save-all".to_owned())
            .spawn(move || {
                let outcome = store.save_all_now(&bodies, layout, &directory, &worker_progress);
                let _requester_gone = sender.send(outcome);
            })?;
        Ok(BulkSave { progress, result })
    }

    fn save_all_now(
        &self,
        bodies: &[SaveBody],
        layout: BodyLayout,
        root: &Path,
        progress: &BulkProgress,
    ) -> io::Result<BulkSummary> {
        self.wait_for_writer()?;
        let mut summary = BulkSummary {
            directory: root.to_path_buf(),
            saved: 0,
            incomplete: 0,
            failed: 0,
            cancelled: false,
        };
        let mut entries = Vec::with_capacity(bodies.len());
        for body in bodies {
            if progress.is_cancelled() {
                summary.cancelled = true;
                break;
            }
            let saved = target_directory(root, &body.target, layout)
                .and_then(|directory| self.write_body(body, &directory));
            match &saved {
                Ok(file) => {
                    summary.saved += 1;
                    summary.incomplete += usize::from(!file.complete());
                    progress.bytes.fetch_add(file.bytes(), Ordering::AcqRel);
                }
                Err(_) => summary.failed += 1,
            }
            entries.push(manifest_entry(body, &saved, root));
            progress.done.fetch_add(1, Ordering::AcqRel);
        }
        write_manifest(root, &manifest(layout, &summary, &entries))?;
        Ok(summary)
    }
}

fn target_directory(root: &Path, target: &str, layout: BodyLayout) -> io::Result<PathBuf> {
    match layout {
        BodyLayout::Flat => Ok(root.to_path_buf()),
        BodyLayout::HostPath => {
            let directory = url_directories(target)
                .iter()
                .fold(root.to_path_buf(), |path, component| path.join(component));
            DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(&directory)?;
            Ok(directory)
        }
    }
}

const fn side_name(side: BodySide) -> &'static str {
    match side {
        BodySide::Request => "request",
        BodySide::Response => "response",
    }
}

const fn format_name(format: SaveFormat) -> &'static str {
    match format {
        SaveFormat::Decoded => "decoded",
        SaveFormat::AsReceived => "as-received",
    }
}

fn manifest_entry(body: &SaveBody, saved: &io::Result<SavedBody>, root: &Path) -> Value {
    let mut entry = json!({
        "record": body.record.value(),
        "side": side_name(body.side),
        "method": body.method,
        "url": body.target,
        "status": body.status,
        "content_type": body.media_type.as_ref().map(crate::engine::domain::MediaType::essence),
        "format": format_name(body.format),
    });
    let details = match saved {
        Ok(file) => json!({
            "file": relative(file.path(), root),
            "bytes": file.bytes(),
            "sha256": file.sha256(),
            "complete": file.complete(),
            "incomplete_reason": file.incomplete_reason(),
        }),
        Err(error) => json!({ "error": error.to_string() }),
    };
    if let (Some(entry), Value::Object(details)) = (entry.as_object_mut(), details) {
        entry.extend(details);
    }
    entry
}

/// Returns `path` relative to `root` with `/` separators.
fn relative(path: &Path, root: &Path) -> String {
    let relative = path.strip_prefix(root).unwrap_or(path);
    relative
        .components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

fn manifest(layout: BodyLayout, summary: &BulkSummary, entries: &[Value]) -> Value {
    let created = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs());
    json!({
        "generator": crate::APP_NAME,
        "created_unix": created,
        "layout": match layout {
            BodyLayout::Flat => "flat",
            BodyLayout::HostPath => "host-path",
        },
        "cancelled": summary.cancelled,
        "saved": summary.saved,
        "incomplete": summary.incomplete,
        "failed": summary.failed,
        "entries": entries,
    })
}

fn write_manifest(root: &Path, manifest: &Value) -> io::Result<()> {
    let mut file = NamedTempFile::new_in(root)?;
    serde_json::to_writer_pretty(file.as_file_mut(), manifest).map_err(io::Error::other)?;
    file.write_all(b"\n")?;
    file.as_file_mut().sync_all()?;
    file.persist(root.join(MANIFEST))
        .map_err(|error| error.error)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::body_store::BodyRecorder;
    use crate::engine::domain::{BodyLifecycle, RecordId};

    fn body(record: u64, target: &str, directory: &Path) -> SaveBody {
        SaveBody {
            record: RecordId::new(record),
            side: BodySide::Response,
            format: SaveFormat::AsReceived,
            directory: directory.to_path_buf(),
            target: target.to_owned(),
            method: "GET".to_owned(),
            status: Some(200),
            disposition: None,
            media_type: None,
            encodings: Vec::new(),
        }
    }

    fn manifest_of(folder: &Path) -> io::Result<Value> {
        serde_json::from_slice(&std::fs::read(folder.join(MANIFEST))?).map_err(io::Error::other)
    }

    #[test]
    fn cancelled_saves_stop_and_still_write_a_manifest() -> io::Result<()> {
        let root = tempfile::tempdir()?;
        let store = BodyStore::with_limits(root.path(), u64::MAX, 0)?;
        let output = tempfile::tempdir()?;
        let folder = output.path().join("bulk");
        DirBuilder::new().mode(0o700).create(&folder)?;
        let progress = BulkProgress {
            total: 1,
            done: AtomicUsize::new(0),
            bytes: AtomicU64::new(0),
            cancelled: AtomicBool::new(true),
        };
        let bodies = [body(1, "http://example.test/a", &folder)];
        let summary = store.save_all_now(&bodies, BodyLayout::Flat, &folder, &progress)?;
        assert!(summary.cancelled());
        assert_eq!(summary.saved(), 0);
        let manifest = manifest_of(&folder)?;
        assert_eq!(manifest.pointer("/cancelled"), Some(&Value::Bool(true)));
        assert_eq!(
            manifest
                .pointer("/entries")
                .and_then(Value::as_array)
                .map(Vec::len),
            Some(0)
        );
        Ok(())
    }

    #[test]
    fn missing_bodies_are_listed_as_errors() -> io::Result<()> {
        let root = tempfile::tempdir()?;
        let store = BodyStore::with_limits(root.path(), u64::MAX, 0)?;
        let mut recorder = BodyRecorder::new(store.clone(), 2, BodySide::Response);
        recorder.observe(b"stored");
        recorder.end(BodyLifecycle::Complete);
        let output = tempfile::tempdir()?;
        let folder = output.path().join("bulk");
        let bodies = vec![
            body(2, "http://example.test/dir/stored.bin", &folder),
            body(3, "http://example.test/missing", &folder),
        ];
        let running = store.save_all(bodies, BodyLayout::HostPath, folder.clone())?;
        let summary = loop {
            if let Some(result) = running.try_finish() {
                break result?;
            }
            thread::yield_now();
        };
        assert_eq!((summary.saved(), summary.failed()), (1, 1));
        let manifest = manifest_of(&folder)?;
        assert_eq!(
            manifest.pointer("/entries/0/file").and_then(Value::as_str),
            Some("example.test/dir/stored.bin")
        );
        assert!(
            manifest
                .pointer("/entries/1/error")
                .is_some_and(Value::is_string)
        );
        assert_eq!(
            std::fs::read(folder.join("example.test/dir/stored.bin"))?,
            b"stored"
        );
        assert!(
            store
                .save_all(Vec::new(), BodyLayout::Flat, folder)
                .is_err(),
            "an existing folder is never reused"
        );
        Ok(())
    }

    #[test]
    fn a_body_named_like_the_manifest_gets_a_numbered_name() -> io::Result<()> {
        let root = tempfile::tempdir()?;
        let store = BodyStore::with_limits(root.path(), u64::MAX, 0)?;
        let mut recorder = BodyRecorder::new(store.clone(), 4, BodySide::Response);
        recorder.observe(b"{}");
        recorder.end(BodyLifecycle::Complete);
        let output = tempfile::tempdir()?;
        let folder = output.path().join("bulk");
        let bodies = vec![body(4, "http://example.test/manifest.json", &folder)];
        let running = store.save_all(bodies, BodyLayout::Flat, folder.clone())?;
        loop {
            if let Some(result) = running.try_finish() {
                result?;
                break;
            }
            thread::yield_now();
        }
        assert_eq!(std::fs::read(folder.join("manifest (1).json"))?, b"{}");
        let manifest = manifest_of(&folder)?;
        assert_eq!(
            manifest.pointer("/entries/0/file").and_then(Value::as_str),
            Some("manifest (1).json")
        );
        Ok(())
    }
}
