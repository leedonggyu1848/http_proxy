//! RFC 8216 HLS playlist parser for multivariant (master) and MPEG-TS media playlists.

use std::fmt;

use thiserror::Error;

use super::url::{HlsUrl, HlsUrlError};

/// Maximum playlist size accepted for in-memory parsing (8 MiB).
pub const MAX_PLAYLIST_BYTES: usize = 8 * 1024 * 1024;

/// A parsed HLS playlist, either multivariant (master) or media.
#[derive(Clone, Debug, PartialEq)]
pub enum Playlist {
    Master(MasterPlaylist),
    Media(MediaPlaylist),
}

/// A multivariant playlist pointing to one or more quality-specific media playlists.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MasterPlaylist {
    pub url: HlsUrl,
    pub variants: Vec<VariantStream>,
}

/// One `#EXT-X-STREAM-INF` variant entry in a master playlist.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VariantStream {
    pub bandwidth: Option<u64>,
    pub average_bandwidth: Option<u64>,
    pub resolution: Option<String>,
    pub frame_rate: Option<String>,
    pub codecs: Option<String>,
    pub name: Option<String>,
    pub raw_uri: String,
    pub url: HlsUrl,
}

impl VariantStream {
    /// Returns a concise human-readable quality label without redundant prose.
    #[must_use]
    pub fn quality_label(&self) -> String {
        match (&self.name, &self.resolution) {
            (Some(name), Some(res)) if !name.eq_ignore_ascii_case(res) => format!("{name} ({res})"),
            (Some(name), _) => name.clone(),
            (None, Some(res)) => res.clone(),
            (None, None) => self
                .url
                .last_path_segment()
                .map_or_else(|| self.raw_uri.clone(), str::to_owned),
        }
    }

    /// Formats the variant bitrate in kbps or Mbps.
    #[must_use]
    pub fn bandwidth_label(&self) -> String {
        let Some(bps) = self.average_bandwidth.or(self.bandwidth) else {
            return "—".to_owned();
        };
        if bps >= 1_000_000 {
            #[expect(
                clippy::cast_precision_loss,
                reason = "Bitrate display only needs two decimal digits in Mbps"
            )]
            let mbps = bps as f64 / 1_000_000.0;
            format!("{mbps:.2} Mbps")
        } else {
            format!("{} kbps", bps / 1_000)
        }
    }
}

/// A media playlist listing ordered media segments.
#[derive(Clone, Debug, PartialEq)]
pub struct MediaPlaylist {
    pub url: HlsUrl,
    pub target_duration_secs: Option<f64>,
    pub media_sequence: u64,
    pub end_list: bool,
    pub segments: Vec<MediaSegment>,
    pub unsupported: Option<UnsupportedHlsFeature>,
}

impl MediaPlaylist {
    /// Total duration of all listed segments in seconds.
    #[must_use]
    pub fn total_duration_secs(&self) -> f64 {
        self.segments.iter().map(|seg| seg.duration_secs).sum()
    }
}

/// One media segment entry (`#EXTINF` + resolved URI).
#[derive(Clone, Debug, PartialEq)]
pub struct MediaSegment {
    pub index: usize,
    pub media_sequence: u64,
    pub duration_secs: f64,
    pub title: Option<String>,
    pub raw_uri: String,
    pub url: HlsUrl,
}

/// HLS feature that this MPEG-TS-first implementation intentionally rejects.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum UnsupportedHlsFeature {
    Encrypted(String),
    InitMap,
    ByteRange,
    NonMpegTsSegment(String),
}

impl fmt::Display for UnsupportedHlsFeature {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Encrypted(method) => {
                write!(f, "encrypted segments (#EXT-X-KEY METHOD={method})")
            }
            Self::InitMap => f.write_str("fMP4 initialization map (#EXT-X-MAP)"),
            Self::ByteRange => f.write_str("byte-range segments (#EXT-X-BYTERANGE)"),
            Self::NonMpegTsSegment(ext) => {
                write!(f, "non-MPEG-TS segment (.{ext})")
            }
        }
    }
}

/// Reason an HLS playlist body could not be parsed.
#[derive(Clone, Debug, Eq, PartialEq, Error)]
pub enum PlaylistParseError {
    #[error("playlist does not start with #EXTM3U")]
    MissingHeader,
    #[error("playlist contains neither variant streams nor media segments")]
    Empty,
    #[error("invalid URI {raw:?} on line {line}: {source}")]
    InvalidUri {
        line: usize,
        raw: String,
        #[source]
        source: HlsUrlError,
    },
    #[error("invalid #EXTINF duration on line {line}: {raw:?}")]
    InvalidExtinf { line: usize, raw: String },
}

/// Parses an HLS playlist text using `base_url` for RFC 8216 Section 4.1 relative URL resolution.
pub fn parse_playlist(base_url: &HlsUrl, text: &str) -> Result<Playlist, PlaylistParseError> {
    let text = text.strip_prefix('\u{FEFF}').unwrap_or(text);
    let mut lines = text.lines().enumerate();
    let Some((_, first)) = lines.find(|(_, line)| !line.trim().is_empty()) else {
        return Err(PlaylistParseError::MissingHeader);
    };
    if first.trim() != "#EXTM3U" {
        return Err(PlaylistParseError::MissingHeader);
    }

    let mut state = ParseState::new(base_url);
    for (zero_idx, raw_line) in lines {
        let line_no = zero_idx + 1;
        let line = raw_line.trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with('#') {
            state.handle_tag(line, line_no)?;
        } else {
            state.handle_uri(line, line_no)?;
        }
    }

    state.finish()
}

struct PendingVariant {
    bandwidth: Option<u64>,
    average_bandwidth: Option<u64>,
    resolution: Option<String>,
    frame_rate: Option<String>,
    codecs: Option<String>,
    name: Option<String>,
}

struct PendingSegment {
    duration_secs: f64,
    title: Option<String>,
}

struct ParseState<'a> {
    base_url: &'a HlsUrl,
    variants: Vec<VariantStream>,
    segments: Vec<MediaSegment>,
    pending_variant: Option<PendingVariant>,
    pending_segment: Option<PendingSegment>,
    target_duration_secs: Option<f64>,
    media_sequence: u64,
    end_list: bool,
    unsupported: Option<UnsupportedHlsFeature>,
}

impl<'a> ParseState<'a> {
    const fn new(base_url: &'a HlsUrl) -> Self {
        Self {
            base_url,
            variants: Vec::new(),
            segments: Vec::new(),
            pending_variant: None,
            pending_segment: None,
            target_duration_secs: None,
            media_sequence: 0,
            end_list: false,
            unsupported: None,
        }
    }

    fn set_unsupported(&mut self, feature: UnsupportedHlsFeature) {
        if self.unsupported.is_none() {
            self.unsupported = Some(feature);
        }
    }

    fn handle_tag(&mut self, line: &str, line_no: usize) -> Result<(), PlaylistParseError> {
        if let Some(attrs) = line.strip_prefix("#EXT-X-STREAM-INF:") {
            self.pending_variant = Some(parse_stream_inf(attrs));
        } else if let Some(rest) = line.strip_prefix("#EXTINF:") {
            self.pending_segment = Some(parse_extinf(rest, line_no)?);
        } else if let Some(val) = line.strip_prefix("#EXT-X-TARGETDURATION:") {
            self.target_duration_secs = val.trim().parse::<f64>().ok();
        } else if let Some(val) = line.strip_prefix("#EXT-X-MEDIA-SEQUENCE:") {
            if let Ok(seq) = val.trim().parse::<u64>() {
                self.media_sequence = seq;
            }
        } else if line == "#EXT-X-ENDLIST" {
            self.end_list = true;
        } else if let Some(attrs) = line.strip_prefix("#EXT-X-KEY:") {
            if let Some(method) = extract_encryption_method(attrs) {
                self.set_unsupported(UnsupportedHlsFeature::Encrypted(method));
            }
        } else if line.starts_with("#EXT-X-MAP:") {
            self.set_unsupported(UnsupportedHlsFeature::InitMap);
        } else if line.starts_with("#EXT-X-BYTERANGE:") {
            self.set_unsupported(UnsupportedHlsFeature::ByteRange);
        }
        Ok(())
    }

    fn handle_uri(&mut self, raw_uri: &str, line_no: usize) -> Result<(), PlaylistParseError> {
        if let Some(pending) = self.pending_variant.take() {
            let url = self.base_url.resolve(raw_uri).map_err(|source| {
                PlaylistParseError::InvalidUri {
                    line: line_no,
                    raw: raw_uri.to_owned(),
                    source,
                }
            })?;
            self.variants.push(VariantStream {
                bandwidth: pending.bandwidth,
                average_bandwidth: pending.average_bandwidth,
                resolution: pending.resolution,
                frame_rate: pending.frame_rate,
                codecs: pending.codecs,
                name: pending.name,
                raw_uri: raw_uri.to_owned(),
                url,
            });
            return Ok(());
        }

        if let Some(pending) = self.pending_segment.take() {
            let url = self.base_url.resolve(raw_uri).map_err(|source| {
                PlaylistParseError::InvalidUri {
                    line: line_no,
                    raw: raw_uri.to_owned(),
                    source,
                }
            })?;
            if let Some(ext) = non_ts_extension(&url) {
                self.set_unsupported(UnsupportedHlsFeature::NonMpegTsSegment(ext));
            }
            let index = self.segments.len();
            let seq = self.media_sequence.saturating_add(index as u64);
            self.segments.push(MediaSegment {
                index,
                media_sequence: seq,
                duration_secs: pending.duration_secs,
                title: pending.title,
                raw_uri: raw_uri.to_owned(),
                url,
            });
        }
        Ok(())
    }

    fn finish(self) -> Result<Playlist, PlaylistParseError> {
        if !self.variants.is_empty() {
            return Ok(Playlist::Master(MasterPlaylist {
                url: self.base_url.clone(),
                variants: self.variants,
            }));
        }
        if !self.segments.is_empty() || self.unsupported.is_some() {
            return Ok(Playlist::Media(MediaPlaylist {
                url: self.base_url.clone(),
                target_duration_secs: self.target_duration_secs,
                media_sequence: self.media_sequence,
                end_list: self.end_list,
                segments: self.segments,
                unsupported: self.unsupported,
            }));
        }
        Err(PlaylistParseError::Empty)
    }
}

/// Returns `Some(ext)` when the segment has a known non-MPEG-TS file extension.
fn non_ts_extension(url: &HlsUrl) -> Option<String> {
    let ext = url.extension()?;
    if ext == "ts" { None } else { Some(ext) }
}

fn parse_stream_inf(attrs: &str) -> PendingVariant {
    let mut variant = PendingVariant {
        bandwidth: None,
        average_bandwidth: None,
        resolution: None,
        frame_rate: None,
        codecs: None,
        name: None,
    };
    for (key, value) in parse_attribute_list(attrs) {
        if key.eq_ignore_ascii_case("BANDWIDTH") {
            variant.bandwidth = value.parse::<u64>().ok();
        } else if key.eq_ignore_ascii_case("AVERAGE-BANDWIDTH") {
            variant.average_bandwidth = value.parse::<u64>().ok();
        } else if key.eq_ignore_ascii_case("RESOLUTION") {
            variant.resolution = Some(value);
        } else if key.eq_ignore_ascii_case("FRAME-RATE") {
            variant.frame_rate = Some(value);
        } else if key.eq_ignore_ascii_case("CODECS") {
            variant.codecs = Some(value);
        } else if key.eq_ignore_ascii_case("NAME") {
            variant.name = Some(value);
        }
    }
    variant
}

fn parse_extinf(rest: &str, line_no: usize) -> Result<PendingSegment, PlaylistParseError> {
    let (dur_raw, title_raw) = rest.split_once(',').unwrap_or((rest, ""));
    let duration_secs =
        dur_raw
            .trim()
            .parse::<f64>()
            .map_err(|_| PlaylistParseError::InvalidExtinf {
                line: line_no,
                raw: rest.to_owned(),
            })?;
    if !duration_secs.is_finite() || duration_secs < 0.0 {
        return Err(PlaylistParseError::InvalidExtinf {
            line: line_no,
            raw: rest.to_owned(),
        });
    }
    let title = title_raw.trim();
    Ok(PendingSegment {
        duration_secs,
        title: (!title.is_empty()).then(|| title.to_owned()),
    })
}

fn extract_encryption_method(attrs: &str) -> Option<String> {
    let method = parse_attribute_list(attrs)
        .into_iter()
        .find(|(key, _)| key.eq_ignore_ascii_case("METHOD"))
        .map(|(_, value)| value)?;
    (!method.eq_ignore_ascii_case("NONE")).then_some(method)
}

/// Parses an RFC 8216 Section 4.2 attribute list (`KEY=VALUE,KEY="QUOTED,VALUE"`).
fn parse_attribute_list(input: &str) -> Vec<(String, String)> {
    let mut pairs = Vec::new();
    let mut rest = input.trim();
    while !rest.is_empty() {
        let Some((key, after_eq)) = rest.split_once('=') else {
            break;
        };
        let key = key.trim().to_owned();
        let after_eq = after_eq.trim_start();
        let (value, remaining) = after_eq.strip_prefix('"').map_or_else(
            || {
                after_eq.split_once(',').map_or_else(
                    || (after_eq.trim().to_owned(), ""),
                    |(val, tail)| (val.trim().to_owned(), tail),
                )
            },
            parse_quoted_attr,
        );
        if !key.is_empty() {
            pairs.push((key, value));
        }
        rest = remaining.trim_start();
        if let Some(after_comma) = rest.strip_prefix(',') {
            rest = after_comma.trim_start();
        }
    }
    pairs
}

fn parse_quoted_attr(quoted: &str) -> (String, &str) {
    quoted.find('"').map_or_else(
        || (quoted.to_owned(), ""),
        |end| {
            (
                quoted.get(..end).unwrap_or_default().to_owned(),
                quoted.get(end + 1..).unwrap_or_default(),
            )
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_master_playlist_with_relative_variants() -> Result<(), Box<dyn std::error::Error>> {
        let base = HlsUrl::parse_absolute("https://cdn.example/video/master.m3u8?token=abc")?;
        let text = "\u{FEFF}#EXTM3U\r\n\
            #EXT-X-VERSION:3\r\n\
            #EXT-X-STREAM-INF:BANDWIDTH=2500000,RESOLUTION=1280x720,CODECS=\"avc1.64001f,mp4a.40.2\"\r\n\
            720p/index.m3u8\r\n\
            #EXT-X-STREAM-INF:BANDWIDTH=800000,RESOLUTION=640x360\r\n\
            ../low/index.m3u8?q=low\r\n";

        let Playlist::Master(master) = parse_playlist(&base, text)? else {
            return Err("expected master playlist".into());
        };
        assert_eq!(master.variants.len(), 2);
        let first = master.variants.first().ok_or("missing first variant")?;
        assert_eq!(first.bandwidth, Some(2_500_000));
        assert_eq!(first.resolution.as_deref(), Some("1280x720"));
        assert_eq!(first.codecs.as_deref(), Some("avc1.64001f,mp4a.40.2"));
        assert_eq!(
            first.url.as_str(),
            "https://cdn.example/video/720p/index.m3u8"
        );
        assert_eq!(first.bandwidth_label(), "2.50 Mbps");

        let second = master.variants.get(1).ok_or("missing second variant")?;
        assert_eq!(
            second.url.as_str(),
            "https://cdn.example/low/index.m3u8?q=low"
        );
        assert_eq!(second.bandwidth_label(), "800 kbps");
        Ok(())
    }

    #[test]
    fn parses_media_playlist_with_relative_ts_segments() -> Result<(), Box<dyn std::error::Error>> {
        let base = HlsUrl::parse_absolute("http://127.0.0.1:9000/live/720p/index.m3u8")?;
        let text = "#EXTM3U\n\
            #EXT-X-VERSION:3\n\
            #EXT-X-TARGETDURATION:4\n\
            #EXT-X-MEDIA-SEQUENCE:10\n\
            #EXTINF:4.000,\n\
            seg0.ts\n\
            #EXT-X-DISCONTINUITY\n\
            #EXTINF:3.5,part two\n\
            ../shared/seg1.ts?sig=1\n\
            #EXT-X-ENDLIST\n";

        let Playlist::Media(media) = parse_playlist(&base, text)? else {
            return Err("expected media playlist".into());
        };
        assert!(media.end_list);
        assert_eq!(media.media_sequence, 10);
        assert_eq!(media.unsupported, None);
        assert_eq!(media.segments.len(), 2);
        let s0 = media.segments.first().ok_or("missing seg 0")?;
        let s1 = media.segments.get(1).ok_or("missing seg 1")?;
        assert_eq!(s0.media_sequence, 10);
        assert_eq!(s0.url.as_str(), "http://127.0.0.1:9000/live/720p/seg0.ts");
        assert_eq!(s1.media_sequence, 11);
        assert_eq!(
            s1.url.as_str(),
            "http://127.0.0.1:9000/live/shared/seg1.ts?sig=1"
        );
        Ok(())
    }

    #[test]
    fn flags_encrypted_fmp4_and_non_ts_playlists_as_unsupported()
    -> Result<(), Box<dyn std::error::Error>> {
        let base = HlsUrl::parse_absolute("https://example.test/index.m3u8")?;

        let enc = "#EXTM3U\n#EXT-X-KEY:METHOD=AES-128,URI=\"key.bin\"\n#EXTINF:4.0,\nseg0.ts\n";
        let Playlist::Media(media) = parse_playlist(&base, enc)? else {
            return Err("expected media".into());
        };
        assert_eq!(
            media.unsupported,
            Some(UnsupportedHlsFeature::Encrypted("AES-128".to_owned()))
        );

        let key_none = "#EXTM3U\n#EXT-X-KEY:METHOD=NONE\n#EXTINF:4.0,\nseg0.ts\n";
        let Playlist::Media(media_ok) = parse_playlist(&base, key_none)? else {
            return Err("expected media".into());
        };
        assert_eq!(media_ok.unsupported, None);

        let fmp4 = "#EXTM3U\n#EXT-X-MAP:URI=\"init.mp4\"\n#EXTINF:4.0,\nseg0.m4s\n";
        let Playlist::Media(media_fmp4) = parse_playlist(&base, fmp4)? else {
            return Err("expected media".into());
        };
        assert_eq!(media_fmp4.unsupported, Some(UnsupportedHlsFeature::InitMap));

        let m4s = "#EXTM3U\n#EXTINF:4.0,\nseg0.m4s\n";
        let Playlist::Media(media_m4s) = parse_playlist(&base, m4s)? else {
            return Err("expected media".into());
        };
        assert_eq!(
            media_m4s.unsupported,
            Some(UnsupportedHlsFeature::NonMpegTsSegment("m4s".to_owned()))
        );
        Ok(())
    }
}
