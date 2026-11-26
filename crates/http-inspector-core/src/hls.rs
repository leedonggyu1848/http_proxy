//! HLS playlist inspection, Chrome `.ts` segment gating, and MPEG-TS media downloading.

mod download;
mod fetch;
mod headers;
mod plan;
mod playlist;
mod url;

pub use download::{
    HlsDownloadConfig, HlsDownloadError, HlsDownloadHandle, HlsDownloadOutcome, HlsDownloadPhase,
    HlsDownloadProgress, HlsDownloadSummary, SegmentStatus, start_hls_download,
};
pub use headers::{SafeRequestHeaders, is_chrome_request, is_chrome_user_agent};
pub use plan::{
    CachedSegment, HlsBlockedReason, HlsInspectError, HlsSessionPlan, HlsVariantEntry,
    MediaDownloadPlan, ReferenceRequest, SegmentPlan, SegmentSource, VariantMediaState,
    inspect_hls_session, is_hls_playlist_record,
};
pub use playlist::{
    MAX_PLAYLIST_BYTES, MasterPlaylist, MediaPlaylist, MediaSegment, Playlist, PlaylistParseError,
    UnsupportedHlsFeature, VariantStream, parse_playlist,
};
pub use url::{HlsUrl, HlsUrlError, HttpScheme, Origin};
