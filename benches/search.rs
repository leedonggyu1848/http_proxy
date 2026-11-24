//! Compares single-word search methods on the data the request filter
//! searches: 2,000 body previews of 8 KiB each, and a full-session log of
//! hundreds of MiB read in blocks.
//!
//! Run with `cargo bench --bench search`.

use std::fmt::Write as _;
use std::hint::black_box;
use std::io::{Read as _, Write as _};

use aho_corasick::AhoCorasick;
use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use http_inspector_analysis::search::{MatchOptions, Matcher};

const RECORDS: usize = 2_000;
const PREVIEW_BYTES: usize = 8 * 1024;
const LOG_BYTES: usize = 256 * 1024 * 1024;
const GZIP_BYTES: usize = 64 * 1024 * 1024;
const BLOCK_BYTES: usize = 64 * 1024;
/// Present once, at the end of the last preview, so every method scans
/// every byte before it finds the match.
const NEEDLE: &str = "OrderId-7F3A9C";
const WORDS: [&str; 16] = [
    "\"id\":",
    "\"Name\":",
    "\"status\":",
    "\"items\":[",
    "\"price\":",
    "true,",
    "false,",
    "null,",
    "\"Created\":",
    "\"user\":",
    "\"Token\":",
    "\"quantity\":",
    "\"Region\":",
    "\"message\":",
    "\"Ok\"",
    "},{",
];

/// Deterministic JSON-like ASCII text with mixed case and no match.
fn text(bytes: usize, seed: u64) -> String {
    let mut state = seed | 1;
    let mut text = String::with_capacity(bytes + 32);
    while text.len() < bytes {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        let index = usize::try_from(state >> 60).unwrap_or(0);
        text.push_str(WORDS.get(index).copied().unwrap_or("x"));
        let _infallible = write!(text, "{}", state >> 48);
    }
    text.truncate(bytes);
    text
}

fn previews() -> Vec<String> {
    let mut previews: Vec<String> = (0..RECORDS)
        .map(|index| text(PREVIEW_BYTES, u64::try_from(index).unwrap_or(0)))
        .collect();
    if let Some(last) = previews.last_mut() {
        last.truncate(PREVIEW_BYTES - NEEDLE.len());
        last.push_str(NEEDLE);
    }
    previews
}

fn aho_corasick() -> AhoCorasick {
    AhoCorasick::builder()
        .ascii_case_insensitive(true)
        .build([NEEDLE])
        .unwrap_or_else(|error| unreachable!("the benchmark needle compiles: {error}"))
}

fn matcher(case_sensitive: bool, regex: bool) -> Matcher {
    Matcher::new(
        NEEDLE,
        MatchOptions {
            case_sensitive,
            regex,
        },
    )
    .unwrap_or_else(|error| unreachable!("the benchmark needle compiles: {error}"))
}

/// Boyer-Moore-Horspool with a 256-entry shift table.
struct Horspool {
    needle: Vec<u8>,
    shift: [usize; 256],
}

impl Horspool {
    fn new(needle: &[u8]) -> Self {
        let mut shift = [needle.len(); 256];
        let last = needle.len().saturating_sub(1);
        for (index, byte) in needle.iter().take(last).enumerate() {
            if let Some(entry) = shift.get_mut(usize::from(*byte)) {
                *entry = last - index;
            }
        }
        Self {
            needle: needle.to_vec(),
            shift,
        }
    }

    fn is_match(&self, haystack: &[u8]) -> bool {
        let length = self.needle.len();
        let mut start = 0;
        while let Some(window) = haystack.get(start..start + length) {
            if window == self.needle.as_slice() {
                return true;
            }
            let last = window.last().copied().unwrap_or(0);
            start += self.shift.get(usize::from(last)).copied().unwrap_or(length);
        }
        false
    }
}

/// Knuth-Morris-Pratt with a failure table.
struct Kmp {
    needle: Vec<u8>,
    failure: Vec<usize>,
}

impl Kmp {
    fn new(needle: &[u8]) -> Self {
        let mut failure = vec![0; needle.len()];
        let mut matched = 0;
        for index in 1..needle.len() {
            let byte = needle.get(index);
            while matched > 0 && needle.get(matched) != byte {
                matched = failure.get(matched - 1).copied().unwrap_or(0);
            }
            if needle.get(matched) == byte {
                matched += 1;
            }
            if let Some(entry) = failure.get_mut(index) {
                *entry = matched;
            }
        }
        Self {
            needle: needle.to_vec(),
            failure,
        }
    }

    fn is_match(&self, haystack: &[u8]) -> bool {
        let mut matched = 0;
        for byte in haystack {
            while matched > 0 && self.needle.get(matched) != Some(byte) {
                matched = self.failure.get(matched - 1).copied().unwrap_or(0);
            }
            if self.needle.get(matched) == Some(byte) {
                matched += 1;
                if matched == self.needle.len() {
                    return true;
                }
            }
        }
        false
    }
}

fn preview_search(criterion: &mut Criterion) {
    let previews = previews();
    let lowercase_needle = NEEDLE.to_lowercase();
    let mut group = criterion.benchmark_group("preview_2000x8KiB");
    group.throughput(Throughput::Bytes(
        u64::try_from(RECORDS * PREVIEW_BYTES).unwrap_or(0),
    ));
    let count =
        |matches: &dyn Fn(&str) -> bool| previews.iter().filter(|text| matches(text)).count();

    group.bench_function("ignore_case/to_lowercase+contains (current)", |bench| {
        bench.iter(|| count(&|text| text.to_lowercase().contains(black_box(&lowercase_needle))));
    });
    let caseless = matcher(false, false);
    group.bench_function("ignore_case/ASCII fold blocks+memmem (selected)", |bench| {
        bench.iter(|| count(&|text| black_box(&caseless).is_match(text.as_bytes())));
    });
    let automaton = aho_corasick();
    group.bench_function("ignore_case/aho-corasick ascii", |bench| {
        bench.iter(|| count(&|text| black_box(&automaton).is_match(text.as_bytes())));
    });
    let caseless_regex = matcher(false, true);
    group.bench_function("ignore_case/regex-automata meta", |bench| {
        bench.iter(|| count(&|text| black_box(&caseless_regex).is_match(text.as_bytes())));
    });

    group.bench_function("match_case/str::contains", |bench| {
        bench.iter(|| count(&|text| text.contains(black_box(NEEDLE))));
    });
    let exact = matcher(true, false);
    group.bench_function("match_case/memchr memmem Finder (selected)", |bench| {
        bench.iter(|| count(&|text| black_box(&exact).is_match(text.as_bytes())));
    });
    let horspool = Horspool::new(NEEDLE.as_bytes());
    group.bench_function("match_case/Horspool", |bench| {
        bench.iter(|| count(&|text| black_box(&horspool).is_match(text.as_bytes())));
    });
    let kmp = Kmp::new(NEEDLE.as_bytes());
    group.bench_function("match_case/KMP", |bench| {
        bench.iter(|| count(&|text| black_box(&kmp).is_match(text.as_bytes())));
    });
    group.finish();
}

/// Searches `log` in blocks, carrying `needle length - 1` bytes into the
/// next block so a match across a block boundary is not missed.
fn blocks_match(log: &[u8], matches: impl Fn(&[u8]) -> bool) -> bool {
    let overlap = NEEDLE.len() - 1;
    let mut window = Vec::with_capacity(BLOCK_BYTES + overlap);
    for block in log.chunks(BLOCK_BYTES) {
        window.extend_from_slice(block);
        if matches(&window) {
            return true;
        }
        let keep = window.len().min(overlap);
        window.drain(..window.len() - keep);
    }
    false
}

fn full_log_search(criterion: &mut Criterion) {
    let log = text(LOG_BYTES, 7).into_bytes();
    let lowercase_needle = NEEDLE.to_lowercase();
    let mut group = criterion.benchmark_group("full_log_256MiB");
    group.sample_size(10);
    group.throughput(Throughput::Bytes(u64::try_from(LOG_BYTES).unwrap_or(0)));
    group.bench_function("ignore_case/to_lowercase+contains per block", |bench| {
        bench.iter(|| {
            blocks_match(&log, |block| {
                String::from_utf8_lossy(block)
                    .to_lowercase()
                    .contains(&lowercase_needle)
            })
        });
    });
    let caseless = matcher(false, false);
    group.bench_function("ignore_case/ASCII fold blocks+memmem (selected)", |bench| {
        bench.iter(|| blocks_match(&log, |block| caseless.is_match(block)));
    });
    let automaton = aho_corasick();
    group.bench_function("ignore_case/aho-corasick ascii", |bench| {
        bench.iter(|| blocks_match(&log, |block| automaton.is_match(block)));
    });
    let exact = matcher(true, false);
    group.bench_function("match_case/memchr memmem Finder", |bench| {
        bench.iter(|| blocks_match(&log, |block| exact.is_match(block)));
    });
    group.finish();
}

fn gzip_stream_search(criterion: &mut Criterion) {
    let plain = text(GZIP_BYTES, 11).into_bytes();
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    let compressed = encoder
        .write_all(&plain)
        .and_then(|()| encoder.finish())
        .unwrap_or_else(|error| unreachable!("in-memory gzip succeeds: {error}"));
    drop(plain);
    let caseless = matcher(false, false);
    let mut group = criterion.benchmark_group("gzip_64MiB_decoded");
    group.sample_size(10);
    group.throughput(Throughput::Bytes(u64::try_from(GZIP_BYTES).unwrap_or(0)));
    group.bench_function("gzip decode+ASCII fold blocks+memmem", |bench| {
        bench.iter(|| {
            let mut decoder = flate2::read::MultiGzDecoder::new(compressed.as_slice());
            let mut block = vec![0; BLOCK_BYTES];
            let mut found = false;
            while let Ok(read) = decoder.read(&mut block) {
                let Some(chunk) = block.get(..read).filter(|chunk| !chunk.is_empty()) else {
                    break;
                };
                found |= caseless.is_match(chunk);
            }
            found
        });
    });
    group.finish();
}

criterion_group!(benches, preview_search, full_log_search, gzip_stream_search);
criterion_main!(benches);
