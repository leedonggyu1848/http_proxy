//! File publication shared by exports and settings.

use std::fs::File;
use std::io;
use std::path::Path;

use tempfile::NamedTempFile;

/// How publication handles an existing destination.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExistingFile {
    /// Keep the destination and return an error if it already exists.
    Preserve,
    /// Replace the destination if it already exists.
    Replace,
}

/// Writes to a temporary file beside the destination and publishes it only
/// after `write` succeeds.
///
/// The parent directory must already exist. The writer is responsible for
/// finishing encoders, flushing buffers, and syncing the file if needed.
/// The published file retains the temporary file's owner-only permissions
/// on Unix.
pub fn publish(
    path: &Path,
    existing: ExistingFile,
    write: impl FnOnce(&mut File) -> io::Result<()>,
) -> io::Result<()> {
    let absolute;
    let path = if path.is_relative() && !path.as_os_str().is_empty() {
        absolute = std::env::current_dir()?.join(path);
        &absolute
    } else {
        path
    };
    let directory = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "Save path has no directory"))?;
    let mut temporary = NamedTempFile::new_in(directory)?;
    write(temporary.as_file_mut())?;
    match existing {
        ExistingFile::Preserve => temporary.persist_noclobber(path),
        ExistingFile::Replace => temporary.persist(path),
    }
    .map(|_| ())
    .map_err(|error| error.error)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::io::{self, Write as _};
    use std::path::Path;

    use super::{ExistingFile, publish};

    #[test]
    fn failed_write_leaves_destination_untouched() -> io::Result<()> {
        let directory = tempfile::tempdir()?;
        let new = directory.path().join("new");
        let existing = directory.path().join("existing");
        fs::write(&existing, b"original")?;
        for (path, policy) in [
            (&new, ExistingFile::Preserve),
            (&existing, ExistingFile::Replace),
        ] {
            let result = publish(path, policy, |file| {
                file.write_all(b"partial")?;
                Err(io::Error::other("write failed"))
            });
            assert!(matches!(result, Err(error) if error.to_string() == "write failed"));
        }
        assert!(!new.exists());
        assert_eq!(fs::read(&existing)?, b"original");
        assert_eq!(fs::read_dir(directory.path())?.count(), 1);
        Ok(())
    }

    #[test]
    fn preserves_file_created_during_write() -> io::Result<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("export");
        let result = publish(&path, ExistingFile::Preserve, |file| {
            file.write_all(b"export")?;
            fs::write(&path, b"other writer")
        });
        assert!(matches!(result, Err(error) if error.kind() == io::ErrorKind::AlreadyExists));
        assert_eq!(fs::read(&path)?, b"other writer");
        assert_eq!(fs::read_dir(directory.path())?.count(), 1);
        Ok(())
    }

    #[test]
    fn replaces_file_at_bare_relative_path() -> io::Result<()> {
        let mut original = tempfile::NamedTempFile::new_in(".")?;
        original.write_all(b"original")?;
        let name = original
            .path()
            .file_name()
            .ok_or_else(|| io::Error::other("temporary file has no name"))?;
        publish(Path::new(name), ExistingFile::Replace, |file| {
            file.write_all(b"replacement")
        })?;
        assert_eq!(fs::read(original.path())?, b"replacement");
        Ok(())
    }

    #[test]
    fn keeps_trailing_dot_in_destination_path() -> io::Result<()> {
        let directory = tempfile::tempdir()?;
        let destination = directory.path().join("export");
        let result = publish(&destination.join("."), ExistingFile::Preserve, |file| {
            file.write_all(b"export")
        });
        assert!(result.is_err());
        assert!(!destination.exists());
        Ok(())
    }
}
