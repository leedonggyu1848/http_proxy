//! UI-boundary localization for domain enums, errors, and notices from `crates/*`.
//!
//! Domain crates intentionally stay language-independent. Every domain type
//! displayed in the UI is translated exhaustively here so new enum variants
//! produce compiler errors until localized for every [`Language`].

use std::io;

use http_inspector_analysis::endpoint_schema::ExclusionReason;
use http_inspector_analysis::filter::RecordFilter;
use http_inspector_analysis::search::{MatcherError, SearchScope};
use http_inspector_analysis::url::IdKind;
use http_inspector_analysis::url_map::{
    ContentTypeFilter, PartyFilter, StatusFilter, TreeSortField,
};
use http_inspector_content::decode::DecodeError;
use http_inspector_content::format::{BodyFormat, ViewMode};
use http_inspector_content::image::ImageError;
use http_inspector_core::https::{CaTrustError, HttpsSetupError};
use http_inspector_core::interception::{StartError, StopError};
use http_inspector_core::launcher::BrowserLaunchError;
use http_inspector_core::recording::{RangePlan, StoreGap, StoredBody};
use http_inspector_core::replay::{
    REPLAY_BODY_FILE_LIMIT_BYTES, ReplayBodyFileError, ReplayError, ReplayHeaderError,
    ReplayUrlError,
};
use http_inspector_core::settings::port::{PortError, PortSaveError, ProxyPort};
use http_inspector_exchange::{BodyLifecycle, FailureStage, TunnelState};
use http_inspector_interop::curl::CurlUnavailable;

use super::{Language, Text};

#[must_use]
pub(crate) const fn record_filter(lang: Language, filter: RecordFilter) -> &'static str {
    match filter {
        RecordFilter::All => lang.pick("All", "전체"),
        RecordFilter::Responses => lang.pick("Responses", "응답"),
        RecordFilter::Failures => lang.pick("Failures", "실패"),
        RecordFilter::Tunnels => lang.pick("Tunnels", "터널"),
    }
}

#[must_use]
pub(crate) const fn search_scope(lang: Language, scope: SearchScope) -> &'static str {
    match scope {
        SearchScope::All => lang.pick("All", "전체"),
        SearchScope::Url => lang.pick("URL", "URL"),
        SearchScope::Headers => lang.pick("Headers", "헤더"),
        SearchScope::RequestBody => lang.pick("Request body", "요청 본문"),
        SearchScope::ResponseBody => lang.pick("Response body", "응답 본문"),
    }
}

#[must_use]
pub(crate) const fn view_mode(lang: Language, mode: ViewMode) -> &'static str {
    match mode {
        ViewMode::Pretty => lang.text(Text::Pretty),
        ViewMode::Raw => lang.text(Text::Raw),
        ViewMode::Tree => lang.text(Text::Tree),
    }
}

#[must_use]
pub(crate) const fn body_format(lang: Language, format: BodyFormat) -> &'static str {
    match format {
        BodyFormat::Json => "JSON",
        BodyFormat::Css => "CSS",
        BodyFormat::Form => lang.pick("Form", "폼"),
        BodyFormat::Multipart => "Multipart",
        BodyFormat::Markup => "HTML",
        BodyFormat::Xml => "XML",
        BodyFormat::JavaScript => "JavaScript",
        BodyFormat::Text => lang.pick("Text", "텍스트"),
    }
}

#[must_use]
pub(crate) const fn status_filter(lang: Language, filter: StatusFilter) -> &'static str {
    match filter {
        StatusFilter::All => lang.pick("All status", "모든 상태"),
        StatusFilter::S2xx => "2xx",
        StatusFilter::S3xx => "3xx",
        StatusFilter::S4xx => "4xx",
        StatusFilter::S5xx => "5xx",
        StatusFilter::Failed => lang.pick("Failed", "실패"),
        StatusFilter::Tunnel => lang.pick("Tunnel", "터널"),
    }
}

#[must_use]
pub(crate) const fn content_type_filter(lang: Language, filter: ContentTypeFilter) -> &'static str {
    match filter {
        ContentTypeFilter::All => lang.pick("All types", "모든 형식"),
        ContentTypeFilter::Html => "HTML",
        ContentTypeFilter::Json => "JSON",
        ContentTypeFilter::Script => lang.pick("Script", "스크립트"),
        ContentTypeFilter::Style => "CSS",
        ContentTypeFilter::Image => lang.pick("Image", "이미지"),
        ContentTypeFilter::Other => lang.pick("Other", "기타"),
    }
}

#[must_use]
pub(crate) const fn party_filter(lang: Language, filter: PartyFilter) -> &'static str {
    match filter {
        PartyFilter::All => lang.pick("All parties", "모든 출처"),
        PartyFilter::FirstParty => lang.pick("1st-party", "퍼스트파티"),
        PartyFilter::ThirdParty => lang.pick("3rd-party", "서드파티"),
    }
}

#[must_use]
pub(crate) const fn tree_sort_field(lang: Language, field: TreeSortField) -> &'static str {
    match field {
        TreeSortField::Name => lang.pick("Name", "이름"),
        TreeSortField::Requests => lang.pick("Requests", "요청"),
        TreeSortField::Errors => lang.pick("Errors", "오류"),
        TreeSortField::Bytes => lang.pick("Size", "크기"),
    }
}

#[must_use]
pub(crate) const fn id_kind(lang: Language, kind: IdKind) -> &'static str {
    match kind {
        IdKind::Integer => lang.pick("integer", "정수"),
        IdKind::Uuid => "UUID",
        IdKind::Hex => lang.pick("hex", "16진수"),
        IdKind::Opaque => lang.pick("opaque id", "식별자"),
    }
}

#[must_use]
pub(crate) const fn exclusion_reason(lang: Language, reason: ExclusionReason) -> &'static str {
    match reason {
        ExclusionReason::Truncated => lang.pick("truncated", "잘림"),
        ExclusionReason::TooLarge => lang.pick("too large", "크기 초과"),
        ExclusionReason::DecodeFailed => lang.pick("decode failed", "디코딩 실패"),
        ExclusionReason::Unavailable => lang.pick("unavailable", "읽기 불가"),
    }
}

#[must_use]
pub(crate) const fn failure_stage(lang: Language, stage: FailureStage) -> &'static str {
    match stage {
        FailureStage::Request => lang.pick("Request", "요청"),
        FailureStage::Connect => lang.pick("Connection", "연결"),
        FailureStage::ConnectTimeout => lang.pick("Connection timeout", "연결 시간 초과"),
        FailureStage::Response => lang.pick("Response", "응답"),
        FailureStage::ResponseTimeout => lang.pick("Response timeout", "응답 시간 초과"),
        FailureStage::Upgrade => lang.pick("Tunnel upgrade", "터널 업그레이드"),
        FailureStage::ClientTls => lang.pick("Client TLS", "클라이언트 TLS"),
        FailureStage::UpstreamTls => lang.pick("Upstream TLS", "업스트림 TLS"),
        FailureStage::Tunnel => lang.pick("Tunnel", "터널"),
    }
}

#[must_use]
pub(crate) fn tunnel_state(lang: Language, state: &TunnelState) -> String {
    match state {
        TunnelState::Open => lang.text(Text::Open).to_owned(),
        TunnelState::Closed => lang.text(Text::Closed).to_owned(),
        TunnelState::Failed { stage, message } => {
            let stage_label = failure_stage(lang, *stage);
            match lang {
                Language::English => format!("Failed at {stage_label}: {message}"),
                Language::Korean => format!("{stage_label} 단계에서 실패: {message}"),
            }
        }
        TunnelState::Incomplete => match lang {
            Language::English => "Incomplete: the proxy stopped while it was open".to_owned(),
            Language::Korean => "불완전: 터널이 열린 상태에서 프록시가 중지되었습니다".to_owned(),
        },
    }
}

#[must_use]
pub(crate) const fn ca_trust_store_description(lang: Language) -> &'static str {
    #[cfg(target_os = "macos")]
    {
        lang.pick(
            "Chrome/Safari (login keychain)",
            "Chrome/Safari (로그인 키체인)",
        )
    }
    #[cfg(not(target_os = "macos"))]
    {
        lang.pick(
            "Chrome/Chromium (current user)",
            "Chrome/Chromium (현재 사용자)",
        )
    }
}

#[must_use]
pub(crate) fn port_error(lang: Language, error: &PortError) -> String {
    match error {
        PortError::Empty => match lang {
            Language::English => "Enter a port number, or 0 to pick a free port.".to_owned(),
            Language::Korean => {
                "포트 번호를 입력하거나, 빈 포트를 자동 선택하려면 0을 입력하세요.".to_owned()
            }
        },
        PortError::Invalid(raw) => match lang {
            Language::English => {
                format!(
                    "\"{raw}\" is not a port. Enter a number from 0 to 65535 (0 picks a free port)."
                )
            }
            Language::Korean => {
                format!(
                    "\"{raw}\"은(는) 유효한 포트가 아닙니다. 0부터 65535 사이의 숫자를 입력하세요 (0은 자동 선택)."
                )
            }
        },
    }
}

#[must_use]
pub(crate) fn port_save_error(lang: Language, error: &PortSaveError) -> String {
    match error {
        PortSaveError::SettingsDirectoryUnavailable => match lang {
            Language::English => {
                "The port could not be saved because the application settings directory is unavailable."
                    .to_owned()
            }
            Language::Korean => {
                "앱 설정 폴더를 찾을 수 없어 포트를 저장하지 못했습니다.".to_owned()
            }
        },
        PortSaveError::Save(source) => match lang {
            Language::English => {
                format!("The proxy started, but the port could not be saved: {source}")
            }
            Language::Korean => {
                format!("프록시는 시작되었지만 포트를 저장하지 못했습니다: {source}")
            }
        },
    }
}

#[must_use]
pub(crate) fn start_error(lang: Language, error: &StartError, port: ProxyPort) -> String {
    match error {
        StartError::Bind(source) if source.kind() == io::ErrorKind::AddrInUse => match lang {
            Language::English => {
                format!(
                    "Port {port} is already in use. Enter another port or 0 to pick a free port."
                )
            }
            Language::Korean => {
                format!(
                    "포트 {port}이(가) 이미 사용 중입니다. 다른 포트를 입력하거나 0을 입력해 빈 포트를 선택하세요."
                )
            }
        },
        StartError::Bind(source) if source.kind() == io::ErrorKind::PermissionDenied => {
            match lang {
                Language::English => {
                    format!("Permission denied for port {port}. Use a port above 1023 or 0.")
                }
                Language::Korean => {
                    format!(
                        "포트 {port}에 대한 권한이 없습니다. 1024 이상의 포트나 0을 사용하세요."
                    )
                }
            }
        }
        StartError::Bind(source) => match lang {
            Language::English => format!("Could not listen on port {port}: {source}"),
            Language::Korean => format!("포트 {port}에서 수신 대기할 수 없습니다: {source}"),
        },
        StartError::ConfigureListener(source) => match lang {
            Language::English => format!("Failed to configure the proxy listener: {source}"),
            Language::Korean => format!("프록시 리스너 설정에 실패했습니다: {source}"),
        },
        StartError::LocalAddress(source) => match lang {
            Language::English => format!("Failed to read the proxy listener address: {source}"),
            Language::Korean => format!("프록시 리스너 주소를 읽지 못했습니다: {source}"),
        },
        StartError::Runtime(source) => match lang {
            Language::English => format!("Failed to create the Tokio runtime: {source}"),
            Language::Korean => format!("Tokio 런타임 생성에 실패했습니다: {source}"),
        },
        StartError::Thread(source) => match lang {
            Language::English => format!("Failed to create the proxy thread: {source}"),
            Language::Korean => format!("프록시 스레드 생성에 실패했습니다: {source}"),
        },
        StartError::Https(https_error) => match lang {
            Language::English => {
                format!(
                    "Failed to prepare HTTPS inspection: {}",
                    https_setup_error(lang, https_error)
                )
            }
            Language::Korean => {
                format!(
                    "HTTPS 복호화 준비에 실패했습니다: {}",
                    https_setup_error(lang, https_error)
                )
            }
        },
    }
}

#[must_use]
pub(crate) fn stop_error(lang: Language, error: &StopError) -> String {
    match error {
        StopError::Thread => match lang {
            Language::English => "The proxy thread panicked.".to_owned(),
            Language::Korean => "프록시 스레드가 비정상 종료되었습니다.".to_owned(),
        },
    }
}

#[must_use]
pub(crate) fn https_setup_error(lang: Language, error: &HttpsSetupError) -> String {
    match error {
        HttpsSetupError::Io {
            operation,
            path,
            source,
        } => match lang {
            Language::English => format!("failed to {operation} {}: {source}", path.display()),
            Language::Korean => format!("{} 작업({operation}) 실패: {source}", path.display()),
        },
        HttpsSetupError::UnsafeDirectory(path) => match lang {
            Language::English => format!(
                "CA directory is not a private regular directory: {}",
                path.display()
            ),
            Language::Korean => format!(
                "CA 폴더가 안전한 개인 일반 디렉터리가 아닙니다: {}",
                path.display()
            ),
        },
        HttpsSetupError::UnsafeIdentity(path) => match lang {
            Language::English => format!(
                "CA identity is not a private regular file: {}",
                path.display()
            ),
            Language::Korean => format!(
                "CA 키 파일이 안전한 개인 일반 파일이 아닙니다: {}",
                path.display()
            ),
        },
        HttpsSetupError::IncompleteIdentity(path) => match lang {
            Language::English => format!(
                "CA identity is incomplete; both files must exist: {}",
                path.display()
            ),
            Language::Korean => format!(
                "CA 파일이 불완전합니다. 인증서와 키 파일이 모두 있어야 합니다: {}",
                path.display()
            ),
        },
        HttpsSetupError::InvalidIdentity(detail) => match lang {
            Language::English => format!("CA identity is invalid: {detail}"),
            Language::Korean => format!("CA 인증서/키가 올바르지 않습니다: {detail}"),
        },
        HttpsSetupError::NoUpstreamRoots => match lang {
            Language::English => "no trusted upstream root certificates were loaded".to_owned(),
            Language::Korean => {
                "신뢰할 수 있는 업스트림 루트 인증서를 불러오지 못했습니다".to_owned()
            }
        },
        HttpsSetupError::InvalidAdditionalRoot(path) => match lang {
            Language::English => format!("additional upstream root is invalid: {}", path.display()),
            Language::Korean => format!(
                "추가 업스트림 루트 인증서가 올바르지 않습니다: {}",
                path.display()
            ),
        },
    }
}

#[must_use]
pub(crate) fn ca_trust_error(lang: Language, error: &CaTrustError) -> String {
    match error {
        CaTrustError::UnsupportedPlatform => match lang {
            Language::English => "CA registration is not supported on this platform.".to_owned(),
            Language::Korean => "이 플랫폼에서는 CA 등록을 지원하지 않습니다.".to_owned(),
        },
        CaTrustError::MissingHome => match lang {
            Language::English => {
                "HOME must name an absolute directory to locate the browser trust store.".to_owned()
            }
            Language::Korean => {
                "브라우저 신뢰 저장소를 찾으려면 HOME에 절대 경로가 설정되어 있어야 합니다."
                    .to_owned()
            }
        },
        CaTrustError::Store(detail) => detail.clone(),
        CaTrustError::Io(source) => match lang {
            Language::English => format!("Failed to access the trust store: {source}"),
            Language::Korean => format!("신뢰 저장소에 접근하지 못했습니다: {source}"),
        },
        CaTrustError::CertificateChanged => match lang {
            Language::English => {
                "Public CA certificate is invalid or has changed; refresh the CA information."
                    .to_owned()
            }
            Language::Korean => {
                "공개 CA 인증서가 유효하지 않거나 변경되었습니다. CA 정보를 새로고침하세요."
                    .to_owned()
            }
        },
    }
}

#[must_use]
pub(crate) fn browser_launch_error(lang: Language, error: &BrowserLaunchError) -> String {
    match error {
        BrowserLaunchError::InvalidProxyAddress => match lang {
            Language::English => {
                "Browser inspection requires a listening loopback proxy.".to_owned()
            }
            Language::Korean => {
                "브라우저 검사를 위해서는 실행 중인 로컬 프록시가 필요합니다.".to_owned()
            }
        },
        BrowserLaunchError::MissingSnapHome => match lang {
            Language::English => "Firefox Snap requires an absolute HOME.".to_owned(),
            Language::Korean => {
                "Firefox Snap을 실행하려면 HOME에 절대 경로가 설정되어 있어야 합니다.".to_owned()
            }
        },
        BrowserLaunchError::ExitedBeforeOpen { browser, status } => match lang {
            Language::English => format!("{browser} exited before opening a window ({status})"),
            Language::Korean => format!("{browser}가 창을 열기 전에 종료되었습니다 ({status})"),
        },
        BrowserLaunchError::ExitedWithStatus { browser, status } => match lang {
            Language::English => format!("{browser} exited with {status}"),
            Language::Korean => format!("{browser}가 종료되었습니다 ({status})"),
        },
        BrowserLaunchError::CaSetup(err) => https_setup_error(lang, err),
        BrowserLaunchError::CaTrust(err) => ca_trust_error(lang, err),
        BrowserLaunchError::Io(err) => err.to_string(),
    }
}

#[must_use]
pub(crate) const fn replay_url_error(lang: Language, error: ReplayUrlError) -> &'static str {
    match error {
        ReplayUrlError::Truncated => lang.pick(
            "The captured URL was truncated; enter the complete URL",
            "캡처된 URL이 잘렸습니다. 전체 URL을 입력하세요",
        ),
        ReplayUrlError::Invalid => lang.pick("Invalid URL", "잘못된 URL입니다"),
        ReplayUrlError::UnsupportedSchemeOrMissingHost => lang.pick(
            "Use an absolute http:// or https:// URL",
            "절대 http:// 또는 https:// URL을 입력하세요",
        ),
        ReplayUrlError::CredentialsOrFragment => lang.pick(
            "URL credentials and fragments are not supported",
            "URL 자격 증명과 프래그먼트(#)는 지원하지 않습니다",
        ),
        ReplayUrlError::TargetsProxy => lang.pick(
            "The URL targets this proxy itself",
            "이 프록시 자체를 대상으로 하는 URL은 보낼 수 없습니다",
        ),
        ReplayUrlError::MissingAuthority => {
            lang.pick("Missing URL authority", "URL 호스트 정보가 없습니다")
        }
        ReplayUrlError::InvalidHost => lang.pick("Invalid Host", "잘못된 Host 값입니다"),
    }
}

#[must_use]
pub(crate) fn replay_header_error(lang: Language, error: &ReplayHeaderError) -> String {
    match error {
        ReplayHeaderError::InvalidName { name } => match lang {
            Language::English => format!("Invalid header name: {name}"),
            Language::Korean => format!("잘못된 헤더 이름: {name}"),
        },
        ReplayHeaderError::RedactedValue => match lang {
            Language::English => "Enter redacted header values or disable their rows".to_owned(),
            Language::Korean => "가려진 헤더의 값을 입력하거나 해당 행을 비활성화하세요".to_owned(),
        },
        ReplayHeaderError::NonUtf8Value { name } => match lang {
            Language::English => format!(
                "Header {name} was captured with non-UTF-8 bytes and cannot be reproduced; re-enter its value or disable the row"
            ),
            Language::Korean => format!(
                "{name} 헤더에 UTF-8이 아닌 바이트가 있어 재현할 수 없습니다. 값을 다시 입력하거나 행을 비활성화하세요"
            ),
        },
        ReplayHeaderError::InvalidValue { name } => match lang {
            Language::English => format!("Invalid value for header {name}"),
            Language::Korean => format!("{name} 헤더의 값이 올바르지 않습니다"),
        },
    }
}

#[must_use]
pub(crate) fn replay_body_file_error(lang: Language, error: &ReplayBodyFileError) -> String {
    let limit_mib = REPLAY_BODY_FILE_LIMIT_BYTES / (1024 * 1024);
    match error {
        ReplayBodyFileError::TooLarge => match lang {
            Language::English => format!("Body files are limited to {limit_mib} MiB"),
            Language::Korean => format!("본문 파일은 최대 {limit_mib} MiB까지 불러올 수 있습니다"),
        },
        ReplayBodyFileError::Io(source) => source.to_string(),
    }
}

#[must_use]
pub(crate) fn replay_error(lang: Language, error: &ReplayError) -> String {
    match error {
        ReplayError::ConnectTunnel => lang
            .pick(
                "CONNECT tunnels cannot be replayed",
                "CONNECT 터널은 재전송할 수 없습니다",
            )
            .to_owned(),
        ReplayError::IncompleteCapture => lang
            .pick(
                "Incomplete body or headers: load a complete replacement before sending",
                "본문이나 헤더가 불완전합니다. 전송 전에 완전한 대체 본문을 불러오세요",
            )
            .to_owned(),
        ReplayError::InvalidMethod => lang
            .pick("Invalid method", "잘못된 HTTP 메서드입니다")
            .to_owned(),
        ReplayError::DisallowedMethod => lang
            .pick(
                "CONNECT and TRACE cannot be replayed",
                "CONNECT와 TRACE 메서드는 재전송할 수 없습니다",
            )
            .to_owned(),
        ReplayError::Url(err) => replay_url_error(lang, *err).to_owned(),
        ReplayError::Header(err) => replay_header_error(lang, err),
        ReplayError::InvalidBodyLength => lang
            .pick("Invalid body length", "잘못된 본문 길이입니다")
            .to_owned(),
        ReplayError::RequestBuild(err) => match lang {
            Language::English => format!("Could not build request: {err}"),
            Language::Korean => format!("요청을 구성하지 못했습니다: {err}"),
        },
        ReplayError::ProxyNotRunning => lang
            .pick(
                "Start the proxy before sending",
                "전송하기 전에 프록시를 시작하세요",
            )
            .to_owned(),
        ReplayError::HttpsInspectionDisabled => lang
            .pick(
                "Enable HTTPS inspection before replaying HTTPS",
                "HTTPS 요청을 재전송하려면 먼저 HTTPS 복호화를 켜세요",
            )
            .to_owned(),
    }
}

#[must_use]
pub(crate) fn matcher_error(lang: Language, error: &MatcherError) -> String {
    match error {
        MatcherError::Empty => lang
            .pick("Search text is empty", "검색어가 비어 있습니다")
            .to_owned(),
        MatcherError::Regex(detail) => match lang {
            Language::English => format!("Invalid regular expression: {detail}"),
            Language::Korean => format!("잘못된 정규식: {detail}"),
        },
    }
}

#[must_use]
pub(crate) fn curl_unavailable(lang: Language, reason: &CurlUnavailable) -> String {
    match reason {
        CurlUnavailable::Tunnel => lang
            .pick(
                "CONNECT tunnels have no HTTP request to copy",
                "CONNECT 터널에는 복사할 HTTP 요청이 없습니다",
            )
            .to_owned(),
        CurlUnavailable::TruncatedUrl => lang
            .pick(
                "The captured URL was truncated, so the command would use another URL",
                "캡처된 URL이 잘려 원본과 같은 URL로 명령을 만들 수 없습니다",
            )
            .to_owned(),
        CurlUnavailable::UnsupportedUrl => lang
            .pick(
                "The request URL is not an absolute http(s) URL",
                "요청 URL이 절대 http(s) URL이 아닙니다",
            )
            .to_owned(),
        CurlUnavailable::OmittedHeaders => lang
            .pick(
                "Some request headers were not captured, so they cannot be copied",
                "일부 요청 헤더가 캡처되지 않아 복사할 수 없습니다",
            )
            .to_owned(),
        CurlUnavailable::UnreproducibleHeader(name) => match lang {
            Language::English => {
                format!("Header {name} was captured with non-UTF-8 bytes and cannot be reproduced")
            }
            Language::Korean => {
                format!("{name} 헤더에 UTF-8이 아닌 바이트가 있어 재현할 수 없습니다")
            }
        },
        CurlUnavailable::BodyInProgress => lang
            .pick(
                "The request body is still being sent; try again when it ends",
                "요청 본문이 아직 전송 중입니다. 완료 후 다시 시도하세요",
            )
            .to_owned(),
        CurlUnavailable::BodyIncomplete => lang
            .pick(
                "The request body ended early, so the original body is unknown",
                "요청 본문이 중간에 끊겨 원본 본문을 알 수 없습니다",
            )
            .to_owned(),
        CurlUnavailable::BodyTruncated { retained, observed } => match lang {
            Language::English => format!(
                "The request body was truncated: only {retained} of {observed} bytes are kept in memory, so the command was not copied"
            ),
            Language::Korean => format!(
                "요청 본문이 잘렸습니다({observed}바이트 중 {retained}바이트만 보관됨). 정확한 명령을 만들 수 없어 복사하지 않았습니다"
            ),
        },
        CurlUnavailable::BodyNotCaptured => lang
            .pick(
                "The request body was not captured, so it cannot be copied",
                "요청 본문이 캡처되지 않아 복사할 수 없습니다",
            )
            .to_owned(),
    }
}

#[must_use]
pub(crate) fn decode_error(lang: Language, error: &DecodeError) -> String {
    match error {
        DecodeError::TooManyLayers => lang
            .pick(
                "Too many Content-Encoding layers to preview safely.",
                "Content-Encoding 중첩이 너무 많아 안전하게 미리볼 수 없습니다.",
            )
            .to_owned(),
        DecodeError::UnsupportedEncoding { encoding } => match lang {
            Language::English => format!(
                "Unsupported Content-Encoding: {encoding:?}. Raw bytes are retained in the full log."
            ),
            Language::Korean => format!(
                "지원하지 않는 Content-Encoding입니다({encoding:?}). 원본 바이트는 전체 로그에 보관됩니다."
            ),
        },
        DecodeError::Init { source } | DecodeError::PrefixDecompress(source) => match lang {
            Language::English => format!("Could not decompress the body: {source}"),
            Language::Korean => format!("본문 압축을 해제하지 못했습니다: {source}"),
        },
        DecodeError::Decompress { encodings, source } => {
            let names = encodings.join(", ");
            match lang {
                Language::English => format!("Could not decompress the body ({names}): {source}"),
                Language::Korean => format!("본문 압축({names})을 해제하지 못했습니다: {source}"),
            }
        }
        DecodeError::LimitExceeded { limit } => {
            let mib = limit / (1024 * 1024);
            match lang {
                Language::English => format!("The decompressed body is larger than {mib} MiB."),
                Language::Korean => format!("압축을 해제한 본문이 {mib} MiB를 초과합니다."),
            }
        }
    }
}

#[must_use]
pub(crate) fn image_error(lang: Language, error: &ImageError) -> String {
    match error {
        ImageError::TooLarge { width, height } => match lang {
            Language::English => format!(
                "The image is {width}×{height} px, larger than the 8192 px per side or 16 megapixel preview limit."
            ),
            Language::Korean => format!(
                "이미지 크기가 {width}×{height} px로 미리보기 한도(한 변 8192 px 또는 1,600만 화소)를 초과합니다."
            ),
        },
        ImageError::TooComplex => lang
            .pick(
                "The SVG has too many elements or nesting levels after expanding <use> references, or a <use> reference cycle.",
                "SVG에 <use> 참조 확장 후 요소나 중첩이 너무 많거나 순환 참조가 있습니다.",
            )
            .to_owned(),
        ImageError::Invalid(format, message) => match lang {
            Language::English => format!("The body is not a valid {} image: {message}", format.name()),
            Language::Korean => format!("올바른 {} 이미지가 아닙니다: {message}", format.name()),
        },
    }
}

#[must_use]
pub(crate) const fn store_gap(lang: Language, gap: StoreGap) -> &'static str {
    match gap {
        StoreGap::QueueFull => lang.pick(
            "the body store queue was full",
            "본문 저장소 대기열이 가득 찼습니다",
        ),
        StoreGap::SessionLimit => lang.pick(
            "the body store reached its session limit",
            "본문 저장소가 세션 한도에 도달했습니다",
        ),
        StoreGap::LowDiskSpace => lang.pick(
            "free disk space was below the minimum",
            "디스크 여유 공간이 최소 기준 미만입니다",
        ),
        StoreGap::WriteFailed => lang.pick(
            "writing to the body store failed",
            "본문 저장소 쓰기에 실패했습니다",
        ),
        StoreGap::StoreClosed => lang.pick("the body store was closed", "본문 저장소가 닫혔습니다"),
    }
}

#[must_use]
pub(crate) fn stored_body_incomplete_reason(lang: Language, stored: &StoredBody) -> Option<String> {
    if let Some(gap) = stored.gap() {
        let reason = store_gap(lang, gap);
        return Some(match lang {
            Language::English => format!("Later bytes are missing because {reason}."),
            Language::Korean => format!("뒤쪽 바이트가 누락되었습니다({reason})."),
        });
    }
    match stored.lifecycle() {
        BodyLifecycle::Complete => None,
        BodyLifecycle::Streaming => Some(
            lang.pick(
                "The body is still being received.",
                "본문을 아직 수신 중입니다.",
            )
            .to_owned(),
        ),
        BodyLifecycle::Interrupted => Some(
            lang.pick(
                "The sender stopped before the body ended.",
                "본문이 끝나기 전에 송신 측 연결이 중단되었습니다.",
            )
            .to_owned(),
        ),
        BodyLifecycle::Cancelled => Some(
            lang.pick(
                "The receiver stopped reading before the body ended.",
                "본문이 끝나기 전에 수신 측에서 읽기를 중단했습니다.",
            )
            .to_owned(),
        ),
    }
}

#[must_use]
pub(crate) fn range_plan_incomplete_reason(lang: Language, plan: &RangePlan) -> Option<String> {
    const LISTED_GAPS: usize = 5;
    let mut reasons = Vec::new();
    let gaps = plan.gaps();
    if !gaps.is_empty() {
        let mut listed: Vec<String> = gaps
            .iter()
            .take(LISTED_GAPS)
            .map(|(first, last)| format!("{first}-{last}"))
            .collect();
        if gaps.len() > LISTED_GAPS {
            let more = gaps.len() - LISTED_GAPS;
            listed.push(match lang {
                Language::English => format!("and {more} more"),
                Language::Korean => format!("외 {more}개 구간"),
            });
        }
        reasons.push(match lang {
            Language::English => format!("Bytes {} were not captured.", listed.join(", ")),
            Language::Korean => format!("{} 바이트 구간이 캡처되지 않았습니다.", listed.join(", ")),
        });
    }
    if plan.total().is_none() {
        reasons.push(
            lang.pick(
                "The server did not send the total length.",
                "서버가 전체 길이를 보내지 않았습니다.",
            )
            .to_owned(),
        );
    }
    if plan.streaming() > 0 {
        let count = plan.streaming();
        reasons.push(match lang {
            Language::English => {
                if count == 1 {
                    "1 matching response is still being received.".to_owned()
                } else {
                    format!("{count} matching responses are still being received.")
                }
            }
            Language::Korean => format!("일치하는 응답 {count}건이 아직 수신 중입니다."),
        });
    }
    (!reasons.is_empty()).then(|| reasons.join(" "))
}

/// Translates body decode/format/image notes produced by domain crates into the active UI language.
#[must_use]
pub(crate) fn localize_content_notice(lang: Language, raw: &str) -> String {
    if lang == Language::English {
        return raw.to_owned();
    }
    if raw == "first 8 KiB of retained bytes" {
        return lang
            .text(Text::DetailBodyNoticeFirst8KiBRetained)
            .to_owned();
    }
    if let Some(rest) = raw.strip_prefix("Content-Encoding: ")
        && let Some((names, detail)) = rest.split_once(" · ")
    {
        let translated_detail = match detail {
            "first 8 KiB of decoded bytes" => lang.text(Text::DetailBodyNoticeFirst8KiBDecoded),
            "complete decoded preview" => lang.text(Text::DetailBodyNoticeCompleteDecoded),
            "decoded prefix; compressed capture is incomplete" => {
                lang.text(Text::DetailBodyNoticeIncompleteDecoded)
            }
            other => other,
        };
        return format!("Content-Encoding: {names} · {translated_detail}");
    }
    if let Some(rest) = raw.strip_prefix("Source map: ")
        && let Some(url) = rest.strip_suffix(" (not fetched).")
    {
        return format!("소스 맵: {url} (가져오지 않음).");
    }
    if raw
        == "Incomplete or invalid CSS; showing the available prefix. CSS property values are not validated."
    {
        return lang.text(Text::DetailBodyNoticeCssIncomplete).to_owned();
    }
    if let Some(rest) = raw.strip_prefix("Showing raw text: could not format as ") {
        return format!("원문으로 표시: 형식을 변환하지 못했습니다({rest})");
    }
    if raw == "text is not drawn; use Source to read it" {
        return lang.text(Text::DetailBodyImageSvgTextNote).to_owned();
    }
    if raw == "embedded images and foreign content are not loaded" {
        return lang.text(Text::DetailBodyImageSvgExternalNote).to_owned();
    }
    if let Some(rest) = raw.strip_prefix("Could not decompress preview (") {
        return format!("미리보기 압축 해제 실패 ({rest}");
    }
    if let Some(rest) = raw.strip_prefix("Could not decompress preview: ") {
        return format!("미리보기 압축 해제 실패: {rest}");
    }
    if raw.starts_with("Too many Content-Encoding layers") {
        return decode_error(lang, &DecodeError::TooManyLayers);
    }
    raw.to_owned()
}
