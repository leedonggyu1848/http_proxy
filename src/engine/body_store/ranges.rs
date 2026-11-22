//! Merging the bodies of several `206 Partial Content` responses into one file.

use std::cmp::Reverse;
use std::fs::File;
use std::io;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};
use std::thread;

use super::export::{self, Part};
use super::file_name::{self, NameSource};
use super::index::{BodyKey, Segment};
use super::{BodyStore, SaveBody, SavedBody, encoding_layers};
use crate::engine::domain::BodyLifecycle;
use crate::engine::journal::BodySide;

/// Gaps listed in an incomplete reason before the rest are counted.
const LISTED_GAPS: usize = 5;

/// The byte range of a single-part `206 Partial Content` body, from its
/// `Content-Range: bytes <start>-<end>/<total>` header.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ContentRange {
    start: u64,
    end: u64,
    total: Option<u64>,
}

impl ContentRange {
    /// Parses `bytes <start>-<end>/<total>` or `bytes <start>-<end>/*`.
    ///
    /// Returns `None` for other units, unsatisfied ranges (`bytes */<total>`),
    /// a start after the end, or an end at or past the total.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        let (unit, rest) = value.trim().split_once(' ')?;
        if !unit.eq_ignore_ascii_case("bytes") {
            return None;
        }
        let (range, total) = rest.trim().split_once('/')?;
        let (start, end) = range.split_once('-')?;
        let start = start.trim().parse::<u64>().ok()?;
        let end = end.trim().parse::<u64>().ok()?;
        let total = match total.trim() {
            "*" => None,
            total => Some(total.parse::<u64>().ok()?),
        };
        (start <= end && total.is_none_or(|total| end < total)).then_some(Self {
            start,
            end,
            total,
        })
    }

    /// Returns the position of the first byte.
    #[must_use]
    pub const fn start(&self) -> u64 {
        self.start
    }

    /// Returns the position of the last byte, inclusive.
    #[must_use]
    pub const fn end(&self) -> u64 {
        self.end
    }

    /// Returns the full length of the resource, when the server sent it.
    #[must_use]
    pub const fn total(&self) -> Option<u64> {
        self.total
    }
}

/// Bytes of one stored part copied to one position of the merged file.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Piece {
    key: BodyKey,
    /// Bytes of the stored body skipped because an earlier part covers them.
    skip: u64,
    /// Position of the first copied byte in the merged file.
    offset: u64,
    length: u64,
}

/// How the `206` responses for one resource fit together.
#[derive(Clone, Debug)]
pub struct RangePlan {
    template: SaveBody,
    total: Option<u64>,
    parts: usize,
    /// Matching responses whose bodies were still arriving.
    streaming: usize,
    pieces: Vec<Piece>,
    gaps: Vec<(u64, u64)>,
}

impl RangePlan {
    /// Returns how many responses contribute bytes.
    #[must_use]
    pub const fn parts(&self) -> usize {
        self.parts
    }

    /// Returns the full length of the resource, when it is known.
    #[must_use]
    pub const fn total(&self) -> Option<u64> {
        self.total
    }

    /// Returns the bytes the parts cover together.
    #[must_use]
    pub fn covered_bytes(&self) -> u64 {
        self.pieces.iter().map(|piece| piece.length).sum()
    }

    /// Returns the missing byte ranges, first and last byte inclusive. A
    /// missing end is listed only when the total length is known.
    #[must_use]
    pub fn gaps(&self) -> &[(u64, u64)] {
        &self.gaps
    }

    /// Returns the validator (`ETag` or `Last-Modified`) the parts share.
    #[must_use]
    pub fn validator(&self) -> Option<&str> {
        self.template.validator.as_deref()
    }

    /// Returns how many matching responses were still being received; their
    /// later bytes are not part of the plan.
    #[must_use]
    pub const fn streaming(&self) -> usize {
        self.streaming
    }

    /// Reports whether the parts cover the whole resource.
    #[must_use]
    pub const fn is_complete(&self) -> bool {
        self.gaps.is_empty() && self.total.is_some()
    }

    /// Explains why the merged file would be incomplete.
    #[must_use]
    pub fn incomplete_reason(&self) -> Option<String> {
        if !self.gaps.is_empty() {
            let mut listed: Vec<String> = self
                .gaps
                .iter()
                .take(LISTED_GAPS)
                .map(|(first, last)| format!("{first}-{last}"))
                .collect();
            if self.gaps.len() > LISTED_GAPS {
                listed.push(format!("and {} more", self.gaps.len() - LISTED_GAPS));
            }
            let streaming = if self.streaming > 0 {
                format!(
                    " {} responses are still being received; merge again after they finish.",
                    self.streaming
                )
            } else {
                String::new()
            };
            return Some(format!(
                "Bytes {} were not captured; they are filled with zeros.{streaming}",
                listed.join(", ")
            ));
        }
        self.total.is_none().then(|| {
            "The server did not send the total length, so the end may be missing.".to_owned()
        })
    }
}

/// Places parts in position order, skipping bytes an earlier part already
/// covers, and lists what is missing. Each part is `(key, range, stored)`.
fn plan_pieces(
    mut parts: Vec<(BodyKey, ContentRange, u64)>,
    total: Option<u64>,
) -> (Vec<Piece>, Vec<(u64, u64)>) {
    // Earlier starts first; for equal starts, the longest part first.
    parts.sort_by_key(|(_, range, stored)| (range.start, Reverse(*stored)));
    let mut pieces = Vec::new();
    let mut gaps = Vec::new();
    let mut next: u64 = 0;
    for (key, range, stored) in parts {
        let available = stored.min(range.end - range.start + 1);
        if available == 0 {
            continue;
        }
        let last = range.start + available - 1;
        if last < next {
            continue;
        }
        let first = range.start.max(next);
        if first > next {
            gaps.push((next, first - 1));
        }
        pieces.push(Piece {
            key,
            skip: first - range.start,
            offset: first,
            length: last - first + 1,
        });
        next = last + 1;
    }
    if let Some(total) = total
        && next < total
    {
        gaps.push((next, total - 1));
    }
    (pieces, gaps)
}

/// Returns the part of `segments` after `skip` bytes, `length` bytes long.
fn slice(segments: &[Segment], mut skip: u64, mut length: u64) -> Vec<Segment> {
    let mut sliced = Vec::new();
    for segment in segments {
        if length == 0 {
            break;
        }
        if skip >= segment.length {
            skip -= segment.length;
            continue;
        }
        let taken = (segment.length - skip).min(length);
        sliced.push(Segment {
            offset: segment.offset + skip,
            length: taken,
        });
        skip = 0;
        length -= taken;
    }
    sliced
}

impl BodyStore {
    /// Plans merging `target` with the other `206` responses in
    /// `candidates` that have the same URL, validator, total length and
    /// content encoding. Returns `None` when `target` is not a single-range
    /// `206` response body.
    ///
    /// Waits briefly for the writer so the plan counts every byte received
    /// so far; the forwarding path never waits for this.
    pub fn plan_ranges(
        &self,
        target: &SaveBody,
        candidates: &[SaveBody],
    ) -> io::Result<Option<RangePlan>> {
        let Some(range) = target.content_range else {
            return Ok(None);
        };
        self.wait_for_writer()?;
        let total = range.total;
        let mut streaming = 0;
        let parts: Vec<_> = candidates
            .iter()
            .chain(std::iter::once(target))
            .filter(|candidate| same_resource(target, candidate))
            .filter_map(|candidate| {
                let range = candidate.content_range?;
                let key = BodyKey {
                    record: candidate.record.value(),
                    side: BodySide::Response,
                };
                let body = self.body(candidate.record, BodySide::Response)?;
                Some((key, (range, body)))
            })
            .collect::<std::collections::HashMap<_, _>>()
            .into_iter()
            .map(|(key, (range, body))| {
                streaming += usize::from(body.lifecycle() == BodyLifecycle::Streaming);
                (key, range, body.stored_bytes())
            })
            .collect();
        let (pieces, gaps) = plan_pieces(parts, total);
        let parts = pieces.len();
        Ok(Some(RangePlan {
            template: target.clone(),
            total,
            parts,
            streaming,
            pieces,
            gaps,
        }))
    }

    /// Writes a merged file in `directory` in the background. Missing bytes
    /// between parts are written as zeros so later parts keep their
    /// positions, and an incomplete file name ends in `.partial`.
    pub fn save_ranges(
        &self,
        plan: RangePlan,
        directory: PathBuf,
    ) -> io::Result<Receiver<io::Result<SavedBody>>> {
        let (sender, receiver) = mpsc::channel();
        let store = self.clone();
        thread::Builder::new()
            .name("body-save-ranges".to_owned())
            .spawn(move || {
                let _requester_gone = sender.send(store.save_ranges_now(&plan, &directory));
            })?;
        Ok(receiver)
    }

    fn save_ranges_now(
        &self,
        plan: &RangePlan,
        directory: &std::path::Path,
    ) -> io::Result<SavedBody> {
        self.wait_for_writer()?;
        let parts = {
            let index = self
                .shared
                .index
                .lock()
                .map_err(|_| io::Error::other("The body store index is unavailable"))?;
            plan.pieces
                .iter()
                .map(|piece| {
                    let (segments, _) = index.segments(piece.key).ok_or_else(|| {
                        io::Error::new(io::ErrorKind::NotFound, "A range part is no longer stored")
                    })?;
                    Ok(Part {
                        offset: piece.offset,
                        segments: slice(&segments, piece.skip, piece.length),
                    })
                })
                .collect::<io::Result<Vec<_>>>()?
        };
        let template = &plan.template;
        let kept = encoding_layers(&template.encodings);
        let complete = plan.is_complete();
        let name = file_name::file_name(&NameSource {
            record: template.record.value(),
            side: BodySide::Response,
            target: &template.target,
            disposition: template.disposition.as_deref(),
            media_type: template.media_type.as_ref(),
            kept_encodings: &kept,
            range: None,
            partial: !complete,
        });
        let data = File::open(&self.shared.data_path)?;
        let (path, bytes, sha256, _) = export::save_parts(&data, parts, directory, &name)?;
        Ok(SavedBody {
            path,
            bytes,
            sha256,
            complete,
            incomplete_reason: plan.incomplete_reason(),
        })
    }
}

/// Reports whether two `206` bodies are parts of the same representation.
fn same_resource(target: &SaveBody, candidate: &SaveBody) -> bool {
    let (Some(range), Some(other)) = (target.content_range, candidate.content_range) else {
        return false;
    };
    candidate.side == BodySide::Response
        && candidate.target == target.target
        && candidate.validator == target.validator
        && other.total == range.total
        && encoding_layers(&candidate.encodings) == encoding_layers(&target.encodings)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(record: u64) -> BodyKey {
        BodyKey {
            record,
            side: BodySide::Response,
        }
    }

    fn range(start: u64, end: u64, total: Option<u64>) -> ContentRange {
        ContentRange { start, end, total }
    }

    #[test]
    fn parses_single_byte_ranges() {
        assert_eq!(
            ContentRange::parse("bytes 0-499/1234"),
            Some(range(0, 499, Some(1234)))
        );
        assert_eq!(
            ContentRange::parse(" BYTES 500-999/* "),
            Some(range(500, 999, None))
        );
        for invalid in [
            "bytes */1234",
            "bytes 5-4/10",
            "bytes 0-10/10",
            "items 0-1/2",
            "bytes 0-x/3",
            "bytes 0-1",
            "",
        ] {
            assert_eq!(ContentRange::parse(invalid), None, "{invalid}");
        }
    }

    #[test]
    fn orders_parts_and_writes_overlaps_once() {
        let parts = vec![
            (key(2), range(400, 999, Some(1000)), 600),
            (key(1), range(0, 499, Some(1000)), 500),
        ];
        let (pieces, gaps) = plan_pieces(parts, Some(1000));
        assert_eq!(
            pieces,
            [
                Piece {
                    key: key(1),
                    skip: 0,
                    offset: 0,
                    length: 500
                },
                Piece {
                    key: key(2),
                    skip: 100,
                    offset: 500,
                    length: 500
                },
            ]
        );
        assert_eq!(gaps, []);
    }

    #[test]
    fn lists_gaps_and_drops_covered_parts() {
        let parts = vec![
            (key(1), range(100, 199, Some(1000)), 100),
            (key(2), range(120, 150, Some(1000)), 31),
            // Only 50 of 100 bytes were stored, so 550-599 is missing too.
            (key(3), range(500, 599, Some(1000)), 50),
        ];
        let (pieces, gaps) = plan_pieces(parts, Some(1000));
        assert_eq!(pieces.len(), 2);
        assert_eq!(gaps, [(0, 99), (200, 499), (550, 999)]);
        let (_, unknown_total) = plan_pieces(vec![(key(1), range(0, 9, None), 10)], None);
        assert_eq!(unknown_total, []);
    }

    #[test]
    fn slices_stored_segments() {
        let segments = [
            Segment {
                offset: 100,
                length: 10,
            },
            Segment {
                offset: 500,
                length: 10,
            },
        ];
        assert_eq!(
            slice(&segments, 5, 10),
            [
                Segment {
                    offset: 105,
                    length: 5
                },
                Segment {
                    offset: 500,
                    length: 5
                },
            ]
        );
        assert_eq!(
            slice(&segments, 12, 100),
            [Segment {
                offset: 502,
                length: 8
            }]
        );
        assert_eq!(slice(&segments, 0, 0), []);
    }
}
