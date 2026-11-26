//! Session-level HLS inspection: matching variants, cached segments, and Chrome `.ts` reference requests.

use std::collections::{HashMap, HashSet};
use std::fmt;

use http::{Method, StatusCode};
use http_inspector_encoding::{EmptyTokens, Encodings};
use http_inspector_exchange::{
    BodyLifecycle, BodySide, BodySource, Exchange, MediaType, Outcome, ReadBodyOutcome, RecordId,
};
use thiserror::Error;

use super::headers::{SafeRequestHeaders, is_chrome_request};
use super::playlist::{
    MAX_PLAYLIST_BYTES, MediaPlaylist, MediaSegment, Playlist, PlaylistParseError,
    UnsupportedHlsFeature, VariantStream, parse_playlist,
};
use super::url::{HlsUrl, HlsUrlError, Origin};
use crate::recording::{BodyStore, PreviewBodies, body_store};

const HLS_MIME_TYPES: &[&str] = &[
    "application/vnd.apple.mpegurl",
    "application/x-mpegurl",
    "audio/mpegurl",
    "audio/x-mpegurl",
];

/// Reports whether `record` is a completed `200 OK` GET response that looks like an HLS playlist.
#[must_use]
pub fn is_hls_playlist_record(record: &Exchange) -> bool {
    if record.request().method() != Method::GET {
        return false;
    }
    let Outcome::Response(response) = record.outcome() else {
        return false;
    };
    if response.status() != StatusCode::OK {
        return false;
    }
    let mime_match = response.media_type().is_some_and(is_hls_media_type);
    let url_match =
        HlsUrl::parse_absolute(record.request().target()).is_ok_and(|url| url.is_m3u8_playlist());
    mime_match || url_match
}

fn is_hls_media_type(media_type: &MediaType) -> bool {
    HLS_MIME_TYPES
        .iter()
        .any(|candidate| media_type.essence().eq_ignore_ascii_case(candidate))
}

/// Reason an HLS playlist record could not be opened for inspection.
#[derive(Clone, Debug, Eq, PartialEq, Error)]
pub enum HlsInspectError {
    #[error("invalid playlist URL: {0}")]
    InvalidUrl(#[from] HlsUrlError),
    #[error("playlist response is not a complete 200 OK body")]
    IncompletePlaylist,
    #[error("playlist body is too large to parse")]
    PlaylistTooLarge,
    #[error("playlist body could not be decoded")]
    DecodeFailed,
    #[error("playlist body is not valid UTF-8")]
    InvalidUtf8,
    #[error("{0}")]
    Parse(#[from] PlaylistParseError),
}

/// Complete inspection result for one opened HLS playlist record.
#[derive(Clone, Debug)]
pub struct HlsSessionPlan {
    pub source_record_id: RecordId,
    pub source_url: HlsUrl,
    pub is_master: bool,
    pub variants: Vec<HlsVariantEntry>,
    pub default_variant: usize,
}

/// One selectable quality variant (or the single stream for a media playlist).
#[derive(Clone, Debug)]
pub struct HlsVariantEntry {
    pub quality_label: String,
    pub bandwidth_label: String,
    pub resolution: Option<String>,
    pub codecs: Option<String>,
    pub playlist_url: HlsUrl,
    pub playlist_record_id: Option<RecordId>,
    pub state: VariantMediaState,
}

impl HlsVariantEntry {
    /// Returns the parsed media plan when this variant's media playlist was captured and parsed.
    #[must_use]
    pub fn media(&self) -> Option<&MediaDownloadPlan> {
        match &self.state {
            VariantMediaState::Ready(plan) => Some(plan),
            VariantMediaState::NotCaptured | VariantMediaState::Failed(_) => None,
        }
    }

    /// Reports whether downloading this variant is currently allowed.
    #[must_use]
    pub fn can_download(&self) -> bool {
        self.media().is_some_and(MediaDownloadPlan::can_download)
    }

    /// Returns why downloading is disabled for this variant, if blocked.
    #[must_use]
    pub fn blocked_reason(&self) -> Option<HlsBlockedReason> {
        match &self.state {
            VariantMediaState::NotCaptured => Some(HlsBlockedReason::PlaylistNotCaptured),
            VariantMediaState::Failed(message) => {
                Some(HlsBlockedReason::PlaylistParseFailed(message.clone()))
            }
            VariantMediaState::Ready(plan) => plan.blocked.clone(),
        }
    }
}

/// Availability of a variant's media playlist within the current capture session.
#[derive(Clone, Debug)]
pub enum VariantMediaState {
    NotCaptured,
    Failed(String),
    Ready(Box<MediaDownloadPlan>),
}

/// Reason a variant cannot currently be downloaded.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HlsBlockedReason {
    PlaylistNotCaptured,
    PlaylistParseFailed(String),
    Unsupported(UnsupportedHlsFeature),
    NoSegments,
    MissingChromeSegmentRequest,
}

impl fmt::Display for HlsBlockedReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PlaylistNotCaptured => f.write_str(
                "Media playlist not captured yet. Play this quality in Chrome, then refresh.",
            ),
            Self::PlaylistParseFailed(err) => write!(f, "Media playlist could not be parsed: {err}"),
            Self::Unsupported(feature) => write!(f, "Unsupported HLS stream: {feature}."),
            Self::NoSegments => f.write_str("Media playlist lists no segments."),
            Self::MissingChromeSegmentRequest => f.write_str(
                "No Chrome .ts segment request captured for this quality. Play this quality in Chrome, then refresh.",
            ),
        }
    }
}

/// The Chrome `.ts` segment request that unlocks downloading for a media playlist.
#[derive(Clone, Debug)]
pub struct ReferenceRequest {
    pub record_id: RecordId,
    pub url: HlsUrl,
    pub origin: Origin,
    pub same_origin_headers: SafeRequestHeaders,
    pub cross_origin_headers: SafeRequestHeaders,
}

/// Metadata needed to reuse a completely stored segment response without re-fetching.
#[derive(Clone, Debug)]
pub struct CachedSegment {
    pub record_id: RecordId,
    pub exchange: Exchange,
    pub encodings: Vec<String>,
    pub observed_bytes: u64,
}

/// Planned source of one segment in a media playlist.
#[derive(Clone, Debug)]
pub enum SegmentSource {
    Cached(CachedSegment),
    Fetch { headers: SafeRequestHeaders },
}

/// Download plan entry for a single ordered segment.
#[derive(Clone, Debug)]
pub struct SegmentPlan {
    pub segment: MediaSegment,
    pub source: SegmentSource,
}

impl SegmentPlan {
    #[must_use]
    pub const fn is_cached(&self) -> bool {
        matches!(self.source, SegmentSource::Cached(_))
    }
}

/// Complete download plan for one captured media playlist.
#[derive(Clone, Debug)]
pub struct MediaDownloadPlan {
    pub playlist: MediaPlaylist,
    pub segments: Vec<SegmentPlan>,
    pub cached_count: usize,
    pub missing_count: usize,
    pub reference: Option<ReferenceRequest>,
    pub blocked: Option<HlsBlockedReason>,
    pub suggested_file_name: String,
}

impl MediaDownloadPlan {
    #[must_use]
    pub const fn can_download(&self) -> bool {
        self.blocked.is_none()
    }
}

/// Inspects `playlist_record` against the current session `records` and optional `body_store`.
///
/// Reads playlist bodies via `BodySource`; call from a background worker rather than the UI thread.
pub fn inspect_hls_session(
    playlist_record: &Exchange,
    records: &[Exchange],
    body_store: Option<&BodyStore>,
) -> Result<HlsSessionPlan, HlsInspectError> {
    let source_url = HlsUrl::parse_absolute(playlist_record.request().target())?;
    let text = read_playlist_text(playlist_record, body_store)?;
    let parsed = parse_playlist(&source_url, &text)?;
    let session = playlist_record.session();
    let session_records: Vec<&Exchange> = records
        .iter()
        .filter(|record| record.session() == session)
        .collect();

    match parsed {
        Playlist::Master(master) => {
            let variants: Vec<HlsVariantEntry> = master
                .variants
                .iter()
                .map(|variant| inspect_variant(variant, &source_url, &session_records, body_store))
                .collect();
            let default_variant = pick_default_variant(&variants);
            Ok(HlsSessionPlan {
                source_record_id: playlist_record.id(),
                source_url,
                is_master: true,
                variants,
                default_variant,
            })
        }
        Playlist::Media(media) => {
            let plan = build_media_plan(media, &source_url, None, &session_records, body_store);
            let entry = HlsVariantEntry {
                quality_label: source_url
                    .last_path_segment()
                    .unwrap_or("Media playlist")
                    .to_owned(),
                bandwidth_label: "—".to_owned(),
                resolution: None,
                codecs: None,
                playlist_url: source_url.clone(),
                playlist_record_id: Some(playlist_record.id()),
                state: VariantMediaState::Ready(Box::new(plan)),
            };
            Ok(HlsSessionPlan {
                source_record_id: playlist_record.id(),
                source_url,
                is_master: false,
                variants: vec![entry],
                default_variant: 0,
            })
        }
    }
}

fn pick_default_variant(variants: &[HlsVariantEntry]) -> usize {
    variants
        .iter()
        .position(HlsVariantEntry::can_download)
        .or_else(|| {
            variants
                .iter()
                .position(|variant| variant.media().is_some())
        })
        .unwrap_or(0)
}

fn inspect_variant(
    variant: &VariantStream,
    source_url: &HlsUrl,
    session_records: &[&Exchange],
    body_store: Option<&BodyStore>,
) -> HlsVariantEntry {
    let quality_label = variant.quality_label();
    let matched_record = find_captured_playlist(session_records, &variant.url, body_store);
    let (playlist_record_id, state) = matched_record.map_or_else(
        || (None, VariantMediaState::NotCaptured),
        |record| {
            let parsed = read_playlist_text(record, body_store).and_then(|text| {
                parse_playlist(&variant.url, &text).map_err(HlsInspectError::from)
            });
            let state = match parsed {
                Ok(Playlist::Media(media)) => VariantMediaState::Ready(Box::new(build_media_plan(
                    media,
                    source_url,
                    Some(&quality_label),
                    session_records,
                    body_store,
                ))),
                Ok(Playlist::Master(_)) => VariantMediaState::Failed(
                    "Nested master playlists are not supported".to_owned(),
                ),
                Err(error) => VariantMediaState::Failed(error.to_string()),
            };
            (Some(record.id()), state)
        },
    );

    HlsVariantEntry {
        quality_label,
        bandwidth_label: variant.bandwidth_label(),
        resolution: variant.resolution.clone(),
        codecs: variant.codecs.clone(),
        playlist_url: variant.url.clone(),
        playlist_record_id,
        state,
    }
}

fn find_captured_playlist<'a>(
    session_records: &[&'a Exchange],
    target_url: &HlsUrl,
    body_store: Option<&BodyStore>,
) -> Option<&'a Exchange> {
    session_records.iter().rev().copied().find(|record| {
        record.request().method() == Method::GET
            && is_complete_ok_response(record, body_store)
            && HlsUrl::parse_absolute(record.request().target()).is_ok_and(|url| &url == target_url)
    })
}

fn build_media_plan(
    playlist: MediaPlaylist,
    source_url: &HlsUrl,
    variant_label: Option<&str>,
    session_records: &[&Exchange],
    body_store: Option<&BodyStore>,
) -> MediaDownloadPlan {
    let segment_urls: HashSet<HlsUrl> = playlist.segments.iter().map(|s| s.url.clone()).collect();
    let reference = find_reference_request(session_records, &segment_urls);
    let cached_by_url = collect_cached_segments(session_records, &segment_urls, body_store);
    let origin_headers = collect_same_origin_headers(session_records, &segment_urls);

    let mut segments = Vec::with_capacity(playlist.segments.len());
    let mut cached_count = 0usize;
    let mut missing_count = 0usize;

    for segment in &playlist.segments {
        let source = cached_by_url.get(&segment.url).map_or_else(
            || {
                missing_count += 1;
                let headers =
                    headers_for_segment(&segment.url.origin(), reference.as_ref(), &origin_headers);
                SegmentSource::Fetch { headers }
            },
            |cached| {
                cached_count += 1;
                SegmentSource::Cached(cached.clone())
            },
        );
        segments.push(SegmentPlan {
            segment: segment.clone(),
            source,
        });
    }

    let blocked = compute_blocked_reason(&playlist, reference.as_ref());
    let suggested_file_name = suggested_output_filename(source_url, &playlist.url, variant_label);

    MediaDownloadPlan {
        playlist,
        segments,
        cached_count,
        missing_count,
        reference,
        blocked,
        suggested_file_name,
    }
}

fn compute_blocked_reason(
    playlist: &MediaPlaylist,
    reference: Option<&ReferenceRequest>,
) -> Option<HlsBlockedReason> {
    if let Some(unsupported) = &playlist.unsupported {
        return Some(HlsBlockedReason::Unsupported(unsupported.clone()));
    }
    if playlist.segments.is_empty() {
        return Some(HlsBlockedReason::NoSegments);
    }
    if reference.is_none() {
        return Some(HlsBlockedReason::MissingChromeSegmentRequest);
    }
    None
}

fn find_reference_request(
    session_records: &[&Exchange],
    segment_urls: &HashSet<HlsUrl>,
) -> Option<ReferenceRequest> {
    let mut fallback: Option<(&Exchange, HlsUrl)> = None;
    for &record in session_records.iter().rev() {
        if !is_matching_chrome_ts_request(record, segment_urls) {
            continue;
        }
        let Ok(url) = HlsUrl::parse_absolute(record.request().target()) else {
            continue;
        };
        if matches!(
            record.outcome(),
            Outcome::Response(response) if response.status() == StatusCode::OK
        ) {
            return Some(build_reference_request(record, url));
        }
        if fallback.is_none() {
            fallback = Some((record, url));
        }
    }
    fallback.map(|(record, url)| build_reference_request(record, url))
}

fn is_matching_chrome_ts_request(record: &Exchange, segment_urls: &HashSet<HlsUrl>) -> bool {
    if record.replay_of().is_some() || record.request().method() != Method::GET {
        return false;
    }
    if !is_chrome_request(record) {
        return false;
    }
    let Ok(url) = HlsUrl::parse_absolute(record.request().target()) else {
        return false;
    };
    if !segment_urls.contains(&url) {
        return false;
    }
    if url.is_ts_segment() {
        return true;
    }
    matches!(
        record.outcome(),
        Outcome::Response(response)
            if response
                .media_type()
                .is_some_and(|mt| mt.essence().eq_ignore_ascii_case("video/mp2t"))
    )
}

fn build_reference_request(record: &Exchange, url: HlsUrl) -> ReferenceRequest {
    let captured = record.request().headers();
    let origin = url.origin();
    ReferenceRequest {
        record_id: record.id(),
        url,
        origin,
        same_origin_headers: SafeRequestHeaders::from_same_origin(captured),
        cross_origin_headers: SafeRequestHeaders::from_cross_origin(captured),
    }
}

/// Maps each origin to allowlisted headers from the latest captured segment request on that origin.
fn collect_same_origin_headers(
    session_records: &[&Exchange],
    segment_urls: &HashSet<HlsUrl>,
) -> HashMap<Origin, SafeRequestHeaders> {
    let mut by_origin = HashMap::new();
    for &record in session_records.iter().rev() {
        if !is_matching_chrome_ts_request(record, segment_urls) {
            continue;
        }
        let Ok(url) = HlsUrl::parse_absolute(record.request().target()) else {
            continue;
        };
        by_origin
            .entry(url.origin())
            .or_insert_with(|| SafeRequestHeaders::from_same_origin(record.request().headers()));
    }
    by_origin
}

fn headers_for_segment(
    segment_origin: &Origin,
    reference: Option<&ReferenceRequest>,
    by_origin: &HashMap<Origin, SafeRequestHeaders>,
) -> SafeRequestHeaders {
    if let Some(reference) = reference
        && &reference.origin == segment_origin
    {
        return reference.same_origin_headers.clone();
    }
    if let Some(same_origin) = by_origin.get(segment_origin) {
        return same_origin.clone();
    }
    reference.map_or_else(SafeRequestHeaders::default, |reference| {
        reference.cross_origin_headers.clone()
    })
}

fn collect_cached_segments(
    session_records: &[&Exchange],
    segment_urls: &HashSet<HlsUrl>,
    body_store: Option<&BodyStore>,
) -> HashMap<HlsUrl, CachedSegment> {
    let mut cached = HashMap::new();
    for &record in session_records.iter().rev() {
        if record.request().method() != Method::GET {
            continue;
        }
        let Ok(url) = HlsUrl::parse_absolute(record.request().target()) else {
            continue;
        };
        if !segment_urls.contains(&url) || cached.contains_key(&url) {
            continue;
        }
        if let Some(segment) = as_cached_segment(record, body_store) {
            cached.insert(url, segment);
        }
    }
    cached
}

fn as_cached_segment(record: &Exchange, body_store: Option<&BodyStore>) -> Option<CachedSegment> {
    let Outcome::Response(response) = record.outcome() else {
        return None;
    };
    if response.status() != StatusCode::OK {
        return None;
    }
    if response.body().lifecycle() != BodyLifecycle::Complete {
        return None;
    }
    let encodings: Vec<String> = response
        .headers()
        .get("content-encoding")
        .map(str::to_owned)
        .collect();
    if Encodings::parse(encodings.iter().map(String::as_str), EmptyTokens::Skip).is_err() {
        return None;
    }
    let preview_complete = response
        .body()
        .snapshot()
        .is_some_and(|snapshot| snapshot.is_complete());
    let store_complete = body_store
        .and_then(|store| store.body(record.id(), BodySide::Response))
        .is_some_and(|stored| stored.is_complete());
    if !preview_complete && !store_complete {
        return None;
    }
    Some(CachedSegment {
        record_id: record.id(),
        exchange: record.clone(),
        encodings,
        observed_bytes: response.body().observed_bytes(),
    })
}

fn is_complete_ok_response(record: &Exchange, body_store: Option<&BodyStore>) -> bool {
    as_cached_segment(record, body_store).is_some()
}

fn read_playlist_text(
    record: &Exchange,
    body_store: Option<&BodyStore>,
) -> Result<String, HlsInspectError> {
    let Outcome::Response(response) = record.outcome() else {
        return Err(HlsInspectError::IncompletePlaylist);
    };
    if response.status() != StatusCode::OK {
        return Err(HlsInspectError::IncompletePlaylist);
    }
    let outcome = body_store.map_or_else(
        || PreviewBodies.read(record, BodySide::Response, MAX_PLAYLIST_BYTES),
        |store| store.read(record, BodySide::Response, MAX_PLAYLIST_BYTES),
    );
    match outcome {
        ReadBodyOutcome::Complete(bytes) => {
            String::from_utf8(bytes).map_err(|_| HlsInspectError::InvalidUtf8)
        }
        ReadBodyOutcome::TooLarge => Err(HlsInspectError::PlaylistTooLarge),
        ReadBodyOutcome::DecodeFailed => Err(HlsInspectError::DecodeFailed),
        ReadBodyOutcome::Incomplete | ReadBodyOutcome::NotFound | ReadBodyOutcome::Unavailable => {
            Err(HlsInspectError::IncompletePlaylist)
        }
    }
}

fn suggested_output_filename(
    source_url: &HlsUrl,
    media_url: &HlsUrl,
    variant_label: Option<&str>,
) -> String {
    let base_stem = source_url
        .last_path_segment()
        .or_else(|| media_url.last_path_segment())
        .map(|seg| seg.rsplit_once('.').map_or(seg, |(stem, _)| stem))
        .and_then(body_store::sanitize_file_name)
        .unwrap_or_else(|| "hls-stream".to_owned());

    let candidate = match variant_label.and_then(body_store::sanitize_file_name) {
        Some(label) if !label.eq_ignore_ascii_case(&base_stem) => {
            format!("{base_stem}-{label}.ts")
        }
        _ => format!("{base_stem}.ts"),
    };
    body_store::sanitize_file_name(&candidate).unwrap_or_else(|| "hls-stream.ts".to_owned())
}

#[cfg(test)]
mod tests {
    use http::{HeaderName, HeaderValue};
    use http_inspector_exchange::{ExchangeBuilder, ResponseFixture, SessionId};

    use super::*;

    const CHROME_UA: &str = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";

    fn response_exchange(
        session: SessionId,
        url: &str,
        ua: Option<&str>,
        referer: Option<&str>,
        content_type: &str,
        body: &[u8],
    ) -> Result<Exchange, Box<dyn std::error::Error>> {
        let mut builder = ExchangeBuilder::http(Method::GET, url)?.session(session);
        if let Some(ua) = ua {
            builder = builder.header(
                HeaderName::from_static("user-agent"),
                HeaderValue::from_str(ua)?,
            );
        }
        if let Some(referer) = referer {
            builder = builder.header(
                HeaderName::from_static("referer"),
                HeaderValue::from_str(referer)?,
            );
        }
        let response = ResponseFixture::new(StatusCode::OK)
            .header(
                HeaderName::from_static("content-type"),
                HeaderValue::from_str(content_type)?,
            )
            .body(body.to_vec(), BodyLifecycle::Complete);
        Ok(builder.respond(response)?)
    }

    #[test]
    fn disables_download_when_only_playlist_or_other_stream_ts_is_captured()
    -> Result<(), Box<dyn std::error::Error>> {
        let session = SessionId::next();
        let playlist_body =
            b"#EXTM3U\n#EXTINF:4.0,\nseg0.ts\n#EXTINF:4.0,\nseg1.ts\n#EXT-X-ENDLIST\n";
        let playlist = response_exchange(
            session,
            "http://cdn.test/video-a/index.m3u8",
            Some(CHROME_UA),
            None,
            "application/vnd.apple.mpegurl",
            playlist_body,
        )?;
        let other_stream_ts = response_exchange(
            session,
            "http://cdn.test/video-b/seg0.ts",
            Some(CHROME_UA),
            Some("http://player.test/b"),
            "video/mp2t",
            b"other-ts",
        )?;
        let non_chrome_ts = response_exchange(
            session,
            "http://cdn.test/video-a/seg0.ts",
            Some("curl/8.7.1"),
            Some("http://player.test/a"),
            "video/mp2t",
            b"curl-ts",
        )?;

        let plan = inspect_hls_session(
            &playlist,
            &[playlist.clone(), other_stream_ts, non_chrome_ts],
            None,
        )?;
        let variant = plan.variants.first().ok_or("missing variant")?;
        assert!(!variant.can_download());
        assert_eq!(
            variant.blocked_reason(),
            Some(HlsBlockedReason::MissingChromeSegmentRequest)
        );
        Ok(())
    }

    #[test]
    fn enables_download_and_marks_cached_segments_when_chrome_ts_exists()
    -> Result<(), Box<dyn std::error::Error>> {
        let session = SessionId::next();
        let master_body = b"#EXTM3U\n\
            #EXT-X-STREAM-INF:BANDWIDTH=2000000,RESOLUTION=1280x720\n\
            720p/index.m3u8\n\
            #EXT-X-STREAM-INF:BANDWIDTH=800000,RESOLUTION=640x360\n\
            360p/index.m3u8\n";
        let media_720 = b"#EXTM3U\n#EXTINF:4.0,\nseg0.ts\n#EXTINF:4.0,\nseg1.ts\n#EXT-X-ENDLIST\n";
        let media_360 = b"#EXTM3U\n#EXTINF:4.0,\nseg0.ts\n#EXT-X-ENDLIST\n";

        let master = response_exchange(
            session,
            "http://cdn.test/live/master.m3u8",
            Some(CHROME_UA),
            None,
            "application/vnd.apple.mpegurl",
            master_body,
        )?;
        let m720 = response_exchange(
            session,
            "http://cdn.test/live/720p/index.m3u8",
            Some(CHROME_UA),
            None,
            "application/vnd.apple.mpegurl",
            media_720,
        )?;
        let m360 = response_exchange(
            session,
            "http://cdn.test/live/360p/index.m3u8",
            Some(CHROME_UA),
            None,
            "application/vnd.apple.mpegurl",
            media_360,
        )?;
        let seg0_720 = response_exchange(
            session,
            "http://cdn.test/live/720p/seg0.ts",
            Some(CHROME_UA),
            Some("http://player.test/watch"),
            "video/mp2t",
            b"TS0",
        )?;

        let plan = inspect_hls_session(&master, &[master.clone(), m720, m360, seg0_720], None)?;
        assert!(plan.is_master);
        assert_eq!(plan.variants.len(), 2);
        assert_eq!(plan.default_variant, 0);

        let v720 = plan.variants.first().ok_or("missing 720p")?;
        assert!(v720.can_download());
        let p720 = v720.media().ok_or("missing 720p plan")?;
        assert_eq!(p720.cached_count, 1);
        assert_eq!(p720.missing_count, 1);
        let seg1 = p720.segments.get(1).ok_or("missing seg1")?;
        let SegmentSource::Fetch { headers } = &seg1.source else {
            return Err("seg1 should require fetch".into());
        };
        assert_eq!(headers.get("referer"), Some("http://player.test/watch"));

        // 360p must NOT use 720p's .ts request as its reference!
        let v360 = plan.variants.get(1).ok_or("missing 360p")?;
        assert!(!v360.can_download());
        assert_eq!(
            v360.blocked_reason(),
            Some(HlsBlockedReason::MissingChromeSegmentRequest)
        );
        Ok(())
    }
}
