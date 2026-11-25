//! Copying one stored body to a user file, as received or decoded.

use std::fmt::Write as _;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::os::unix::fs::FileExt;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use tempfile::NamedTempFile;

use super::file_name::numbered;
use super::index::Segment;
use http_inspector_encoding::{EmptyTokens, Encodings, ParseError};
use http_inspector_exchange::ReadBodyOutcome;

const BROTLI_BUFFER: usize = 64 * 1024;
/// Decoded output is always allowed up to this size.
const DECODED_FLOOR_BYTES: u64 = 256 * 1024 * 1024;
/// Decoded output may otherwise be this many times the encoded size.
const DECODED_RATIO: u64 = 100;
/// Decoded output never exceeds this size, to stop decompression bombs.
const DECODED_CEILING_BYTES: u64 = 16 * 1024 * 1024 * 1024;
/// Names tried before a save gives up on finding a free one.
const MAX_NAME_ATTEMPTS: u32 = 10_000;

/// How a body is written to its file.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SaveFormat {
    /// Remove `gzip`, `deflate`, `br` and `zstd` content encodings so the
    /// file holds the actual document, image or media.
    #[default]
    Decoded,
    /// Keep the bytes exactly as they were received, content encoding included.
    AsReceived,
}

pub(super) fn storage_encodings(values: &[String]) -> io::Result<Encodings> {
    Encodings::parse(values.iter().map(String::as_str), EmptyTokens::Skip).map_err(|error| {
        match error {
            ParseError::TooManyLayers => io::Error::new(
                io::ErrorKind::InvalidData,
                "Too many Content-Encoding layers to decode safely; save the body as received",
            ),
            ParseError::Unsupported(value) => io::Error::new(
                io::ErrorKind::Unsupported,
                format!("Content-Encoding {value:?} cannot be decoded; save the body as received"),
            ),
        }
    })
}

/// Reads stored segments of the data file in order, as one stream.
struct SegmentReader {
    file: File,
    segments: std::vec::IntoIter<Segment>,
    current: Option<Segment>,
}

impl SegmentReader {
    fn new(file: File, segments: Vec<Segment>) -> Self {
        Self {
            file,
            segments: segments.into_iter(),
            current: None,
        }
    }
}

impl Read for SegmentReader {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        loop {
            let segment = match self.current {
                Some(segment) if segment.length > 0 => segment,
                _ => match self.segments.next() {
                    Some(segment) => {
                        self.current = Some(segment);
                        continue;
                    }
                    None => return Ok(0),
                },
            };
            let wanted = usize::try_from(segment.length)
                .unwrap_or(usize::MAX)
                .min(buffer.len());
            let read = self
                .file
                .read_at(buffer.get_mut(..wanted).unwrap_or_default(), segment.offset)?;
            if read == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "The body store data file ended early",
                ));
            }
            let advanced = u64::try_from(read).unwrap_or(u64::MAX);
            self.current = Some(Segment {
                offset: segment.offset.saturating_add(advanced),
                length: segment.length.saturating_sub(advanced),
            });
            return Ok(read);
        }
    }
}

/// What to copy and how.
pub(super) struct CopyPlan {
    pub(super) data: File,
    pub(super) segments: Vec<Segment>,
    pub(super) stored_bytes: u64,
    /// Layers to remove, in the order they were applied.
    pub(super) decode: Encodings,
    /// Whether the stored body is known to be incomplete; decoding then
    /// keeps what could be decoded instead of failing at the cut.
    pub(super) incomplete: bool,
}

/// A body file that was written: its path, size, SHA-256 in lowercase
/// hexadecimal, and the numbering attempt that gave its name (0 for `name`
/// itself, `n` for `name (n)`).
pub(super) type Written = (PathBuf, u64, String, u32);

/// Writes the body to a new file in `directory` named `name`, or a numbered
/// variant when that name exists. Returns the path, the bytes written and
/// their SHA-256 in lowercase hexadecimal.
///
/// Numbering starts at `first_attempt`, so a caller saving many files with
/// the same name can skip numbers it already used.
pub(super) fn save(
    copy: CopyPlan,
    directory: &Path,
    name: &str,
    first_attempt: u32,
) -> io::Result<Written> {
    let mut destination = NamedTempFile::new_in(directory)?;
    let mut output = HashingWriter {
        inner: destination.as_file_mut(),
        hasher: Sha256::new(),
    };
    let written = write_body(copy, &mut output)?;
    let digest = output.hasher.finalize();
    destination.as_file_mut().sync_all()?;
    let (path, attempt) = persist_unique(destination, directory, name, first_attempt)?;
    Ok((path, written, hex(&digest), attempt))
}

/// Stored bytes copied to one position of a merged file.
pub(super) struct Part {
    pub(super) offset: u64,
    pub(super) segments: Vec<Segment>,
}

/// Writes `parts` in position order to a new file in `directory`, filling
/// the space between parts with zeros, and names it like [`save`].
pub(super) fn save_parts(
    data: &File,
    parts: Vec<Part>,
    directory: &Path,
    name: &str,
) -> io::Result<Written> {
    let mut destination = NamedTempFile::new_in(directory)?;
    let mut hasher = Sha256::new();
    let mut position: u64 = 0;
    for part in parts {
        let zeros = part.offset.saturating_sub(position);
        if zeros > 0 {
            // Seek over the gap instead of writing zeros: the file system
            // keeps a hole that reads as zeros. Only the hash sees them.
            hash_zeros(&mut hasher, zeros);
            position = part.offset;
            destination.as_file_mut().seek(SeekFrom::Start(position))?;
        }
        let length: u64 = part.segments.iter().map(|segment| segment.length).sum();
        let mut output = io::BufWriter::new(HashingWriter {
            inner: destination.as_file_mut(),
            hasher,
        });
        let copied = io::copy(
            &mut SegmentReader::new(data.try_clone()?, part.segments),
            &mut output,
        )?;
        hasher = output
            .into_inner()
            .map_err(io::IntoInnerError::into_error)?
            .hasher;
        if copied != length {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "The body store data file ended early",
            ));
        }
        position = position.saturating_add(copied);
    }
    // A file that ends in a hole still needs its full length.
    destination.as_file_mut().set_len(position)?;
    destination.as_file_mut().sync_all()?;
    let digest = hasher.finalize();
    let (path, attempt) = persist_unique(destination, directory, name, 0)?;
    Ok((path, position, hex(&digest), attempt))
}

/// Feeds `count` zero bytes to `hasher` without allocating them all.
fn hash_zeros(hasher: &mut Sha256, mut count: u64) {
    let zeros = [0_u8; 8 * 1024];
    while count > 0 {
        let chunk = usize::try_from(count)
            .unwrap_or(zeros.len())
            .min(zeros.len());
        hasher.update(zeros.get(..chunk).unwrap_or_default());
        count -= chunk as u64;
    }
}

fn hex(digest: &[u8]) -> String {
    digest.iter().fold(String::new(), |mut hex, byte| {
        let _infallible = write!(hex, "{byte:02x}");
        hex
    })
}

/// Passes writes through and hashes exactly the bytes that were accepted.
struct HashingWriter<W> {
    inner: W,
    hasher: Sha256,
}

impl<W: Write> Write for HashingWriter<W> {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        let written = self.inner.write(buffer)?;
        self.hasher
            .update(buffer.get(..written).unwrap_or_default());
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

fn write_body(copy: CopyPlan, output: &mut impl Write) -> io::Result<u64> {
    let CopyPlan {
        data,
        segments,
        stored_bytes,
        decode,
        incomplete,
    } = copy;
    let raw = SegmentReader::new(data, segments);
    if decode.is_empty() {
        return io::copy(&mut io::BufReader::new(raw), output);
    }
    let limit = DECODED_FLOOR_BYTES
        .max(stored_bytes.saturating_mul(DECODED_RATIO))
        .min(DECODED_CEILING_BYTES);
    let probe = limit.saturating_add(1);
    let reader = decode.reader(io::BufReader::new(raw), probe, BROTLI_BUFFER)?;
    let mut output = io::BufWriter::new(output);
    let copied = copy_decoded(&mut reader.take(probe), &mut output, incomplete);
    output.flush()?;
    let written = copied?;
    if written > limit {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "The decoded body is larger than {limit} bytes, which looks like a decompression bomb; save it as received"
            ),
        ));
    }
    Ok(written)
}

/// Copies decoded bytes. An incomplete body ends mid-stream, so a decoding
/// error then ends the copy and keeps the bytes decoded so far.
fn copy_decoded(
    reader: &mut impl Read,
    output: &mut impl Write,
    incomplete: bool,
) -> io::Result<u64> {
    let mut buffer = vec![0; 64 * 1024];
    let mut written: u64 = 0;
    loop {
        let read = match reader.read(&mut buffer) {
            Ok(0) => return Ok(written),
            Ok(read) => read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) if incomplete => return Ok(written),
            Err(error) => {
                return Err(io::Error::new(
                    error.kind(),
                    format!("The body could not be decoded ({error}); save it as received"),
                ));
            }
        };
        output.write_all(buffer.get(..read).unwrap_or_default())?;
        written = written.saturating_add(u64::try_from(read).unwrap_or(u64::MAX));
    }
}

/// Reads stored body segments and removes `decode` layers up to `max_decoded_bytes`.
pub(super) fn read_decoded_segments(
    data: File,
    segments: Vec<Segment>,
    decode: &Encodings,
    max_decoded_bytes: usize,
) -> ReadBodyOutcome {
    let reader = io::BufReader::new(SegmentReader::new(data, segments));
    read_decoded_bounded(reader, decode, max_decoded_bytes)
}

/// Decodes an in-memory complete body buffer up to `max_decoded_bytes`.
pub(super) fn decode_complete_bytes(
    bytes: &[u8],
    encodings: &[String],
    max_decoded_bytes: usize,
) -> ReadBodyOutcome {
    let Ok(decode) = storage_encodings(encodings) else {
        return ReadBodyOutcome::DecodeFailed;
    };
    if decode.is_empty() {
        return if bytes.len() > max_decoded_bytes {
            ReadBodyOutcome::TooLarge
        } else {
            ReadBodyOutcome::Complete(bytes.to_vec())
        };
    }
    read_decoded_bounded(bytes, &decode, max_decoded_bytes)
}

fn read_decoded_bounded(
    reader: impl Read,
    decode: &Encodings,
    max_decoded_bytes: usize,
) -> ReadBodyOutcome {
    let limit = u64::try_from(max_decoded_bytes).unwrap_or(u64::MAX);
    let probe = limit.saturating_add(1);
    let Ok(reader) = decode.reader(reader, probe, BROTLI_BUFFER) else {
        return ReadBodyOutcome::DecodeFailed;
    };
    let mut out = Vec::new();
    let mut bounded = reader.take(probe);
    if bounded.read_to_end(&mut out).is_err() {
        return if decode.is_empty() {
            ReadBodyOutcome::Unavailable
        } else {
            ReadBodyOutcome::DecodeFailed
        };
    }
    if out.len() > max_decoded_bytes {
        return ReadBodyOutcome::TooLarge;
    }
    ReadBodyOutcome::Complete(out)
}

/// Moves the finished file to `name`, or `name (1)`, `name (2)`, …, never
/// replacing an existing file.
fn persist_unique(
    mut file: NamedTempFile,
    directory: &Path,
    name: &str,
    first_attempt: u32,
) -> io::Result<(PathBuf, u32)> {
    for attempt in first_attempt..first_attempt.saturating_add(MAX_NAME_ATTEMPTS) {
        let candidate = if attempt == 0 {
            name.to_owned()
        } else {
            numbered(name, attempt)
        };
        let path = directory.join(candidate);
        match file.persist_noclobber(&path) {
            Ok(_) => return Ok((path, attempt)),
            Err(error) if error.error.kind() == io::ErrorKind::AlreadyExists => file = error.file,
            Err(error) => return Err(error.error),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        format!("Too many files named like {name:?} already exist"),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Saves with numbering from the start, as a single save does.
    fn save(copy: CopyPlan, directory: &Path, name: &str) -> io::Result<Written> {
        super::save(copy, directory, name, 0)
    }

    fn data_file(bytes: &[u8]) -> io::Result<(NamedTempFile, File)> {
        let mut file = NamedTempFile::new()?;
        file.write_all(bytes)?;
        file.flush()?;
        let reader = file.reopen()?;
        Ok((file, reader))
    }

    fn copy(data: File, segments: Vec<Segment>, decode: Encodings) -> CopyPlan {
        let stored_bytes = segments.iter().map(|segment| segment.length).sum();
        CopyPlan {
            data,
            segments,
            stored_bytes,
            decode,
            incomplete: false,
        }
    }

    fn gzip(bytes: &[u8]) -> io::Result<Vec<u8>> {
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(bytes)?;
        encoder.finish()
    }

    #[test]
    fn reads_interleaved_segments_in_order() -> io::Result<()> {
        let (_file, data) = data_file(b"AAxxBBByyC")?;
        let segments = vec![
            Segment {
                offset: 0,
                length: 2,
            },
            Segment {
                offset: 4,
                length: 3,
            },
            Segment {
                offset: 9,
                length: 1,
            },
        ];
        let mut text = String::new();
        SegmentReader::new(data, segments).read_to_string(&mut text)?;
        assert_eq!(text, "AABBBC");
        Ok(())
    }

    #[test]
    fn reports_a_truncated_data_file() -> io::Result<()> {
        let (_file, data) = data_file(b"abc")?;
        let mut reader = SegmentReader::new(
            data,
            vec![Segment {
                offset: 1,
                length: 9,
            }],
        );
        let mut bytes = Vec::new();
        let error = reader.read_to_end(&mut bytes).err();
        assert_eq!(
            error.map(|error| error.kind()),
            Some(io::ErrorKind::UnexpectedEof)
        );
        Ok(())
    }

    #[test]
    fn saves_raw_and_decoded_bytes_without_replacing_files() -> io::Result<()> {
        let original: Vec<u8> = (0..=255).cycle().take(100_000).collect();
        let encoded = gzip(&original)?;
        let (_file, data) = data_file(&encoded)?;
        let directory = tempfile::tempdir()?;
        let whole = || {
            vec![Segment {
                offset: 0,
                length: encoded.len() as u64,
            }]
        };
        let (raw, raw_bytes, _, _) = save(
            copy(data.try_clone()?, whole(), Encodings::default()),
            directory.path(),
            "body.gz",
        )?;
        assert_eq!(std::fs::read(&raw)?, encoded);
        assert_eq!(raw_bytes, encoded.len() as u64);
        let decode = storage_encodings(&["gzip".to_owned()])?;
        let (decoded, decoded_bytes, _, _) = save(
            copy(data.try_clone()?, whole(), decode.clone()),
            directory.path(),
            "body",
        )?;
        assert_eq!(std::fs::read(&decoded)?, original);
        assert_eq!(decoded_bytes, original.len() as u64);
        let (again, _, _, _) = save(copy(data, whole(), decode), directory.path(), "body")?;
        assert_eq!(again, directory.path().join("body (1)"));
        assert_eq!(std::fs::read(directory.path().join("body"))?, original);
        Ok(())
    }

    #[test]
    fn decode_errors_fail_complete_bodies_but_keep_incomplete_prefixes() -> io::Result<()> {
        let original = b"hello world, hello world, hello world".repeat(100);
        let encoded = gzip(&original)?;
        let cut = encoded.len() / 2;
        let (_file, data) = data_file(encoded.get(..cut).unwrap_or_default())?;
        let directory = tempfile::tempdir()?;
        let segments = || {
            vec![Segment {
                offset: 0,
                length: cut as u64,
            }]
        };
        let complete = copy(
            data.try_clone()?,
            segments(),
            storage_encodings(&["gzip".to_owned()])?,
        );
        assert!(save(complete, directory.path(), "cut").is_err());
        let mut partial = copy(data, segments(), storage_encodings(&["gzip".to_owned()])?);
        partial.incomplete = true;
        let (path, _, _, _) = save(partial, directory.path(), "cut.partial")?;
        let kept = std::fs::read(path)?;
        assert!(original.starts_with(&kept));
        Ok(())
    }

    #[test]
    fn decodes_brotli_and_zstd() -> io::Result<()> {
        const EXPECTED: &[u8] = include_bytes!("../../../../../tests/fixtures/body-preview.json");
        const BROTLI: &[u8] = include_bytes!("../../../../../tests/fixtures/body-preview.json.br");
        let directory = tempfile::tempdir()?;
        let (_file, data) = data_file(BROTLI)?;
        let segments = vec![Segment {
            offset: 0,
            length: BROTLI.len() as u64,
        }];
        let (path, _, _, _) = save(
            copy(data, segments, storage_encodings(&["br".to_owned()])?),
            directory.path(),
            "br",
        )?;
        assert_eq!(std::fs::read(path)?, EXPECTED);
        let zstd = zstd::encode_all(EXPECTED, 3)?;
        let (_file, data) = data_file(&zstd)?;
        let segments = vec![Segment {
            offset: 0,
            length: zstd.len() as u64,
        }];
        let (path, _, _, _) = save(
            copy(data, segments, storage_encodings(&["zstd".to_owned()])?),
            directory.path(),
            "zst",
        )?;
        assert_eq!(std::fs::read(path)?, EXPECTED);
        Ok(())
    }

    #[test]
    fn merged_gaps_read_as_zeros_and_are_hashed() -> io::Result<()> {
        let (_file, data) = data_file(b"AAAABBBB")?;
        let directory = tempfile::tempdir()?;
        let parts = vec![
            Part {
                offset: 0,
                segments: vec![Segment {
                    offset: 0,
                    length: 4,
                }],
            },
            Part {
                offset: 100_000,
                segments: vec![Segment {
                    offset: 4,
                    length: 4,
                }],
            },
        ];
        let (path, bytes, sha256, _) = save_parts(&data, parts, directory.path(), "merged")?;
        let mut expected = b"AAAA".to_vec();
        expected.resize(100_000, 0);
        expected.extend_from_slice(b"BBBB");
        assert_eq!(bytes, 100_004);
        assert_eq!(std::fs::read(path)?, expected);
        assert_eq!(sha256, hex(&Sha256::digest(&expected)));
        Ok(())
    }
}
