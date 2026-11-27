//! Manual request replay editor UI text catalog.

use crate::ui::i18n::Language;

define_catalog! {
    pub(crate) struct ReplayText {
        update_host_from_url,
        headers,
        cookies,
        add_cookie,
        add_cookie_header,
        query_params,
        add_parameter,
        body,
        valid_json,
        form_fields,
        add_field,
        body_file_hint,
        load_complete_body_file,
        capture_incomplete_warning,
        confirm_state_change,
        skip_confirmation,
        send,
        reset_to_original,
        compare_with_original,
        send_to_compare,
        name_hint,
        value_hint,
        add_header,
        partial_body_preview_suffix,
        preview_updating,
    }
    tooltips: pub(crate) struct ReplayTooltips {
        headers_hint,
    }
    en = {
        update_host_from_url: "Update Host from URL",
        headers: "Headers",
        cookies: "Cookies",
        add_cookie: "Add cookie",
        add_cookie_header: "Add Cookie header",
        query_params: "Query parameters (URL-encoded)",
        add_parameter: "Add parameter",
        body: "Body",
        valid_json: "Valid JSON",
        form_fields: "Form fields (URL-encoded)",
        add_field: "Add field",
        body_file_hint: "Body file path",
        load_complete_body_file: "Load complete body file",
        capture_incomplete_warning: "Capture incomplete; load the full body file, and capture again for omitted headers.",
        confirm_state_change: "I confirm this request may change server state",
        skip_confirmation: "Do not ask again while this editor is open",
        send: "Send",
        reset_to_original: "Reset to original",
        compare_with_original: "Compare with original",
        send_to_compare: "Send a request to compare its captured result.",
        name_hint: "Name",
        value_hint: "Value",
        add_header: "Add header",
        partial_body_preview_suffix: "\n[Partial body preview]",
        preview_updating: "[Preview is updating]",
    }
    en_tooltips = {
        headers_hint: "Cookies are written as Cookie: name=value; name2=value2, hop-by-hop headers are removed, and Content-Length is recalculated.",
    }
    ko = {
        update_host_from_url: "URL에 맞춰 Host 자동 갱신",
        headers: "헤더",
        cookies: "쿠키",
        add_cookie: "쿠키 추가",
        add_cookie_header: "Cookie 헤더 추가",
        query_params: "쿼리 파라미터 (URL 인코딩)",
        add_parameter: "파라미터 추가",
        body: "본문",
        valid_json: "유효한 JSON",
        form_fields: "폼 필드 (URL 인코딩)",
        add_field: "필드 추가",
        body_file_hint: "본문 파일 경로",
        load_complete_body_file: "전체 본문 파일 불러오기",
        capture_incomplete_warning: "캡처가 불완전하므로 전체 본문 파일을 불러오고, 누락된 헤더는 다시 캡처하세요.",
        confirm_state_change: "이 요청이 서버 상태를 변경할 수 있음을 확인했습니다",
        skip_confirmation: "이 편집기가 열려 있는 동안 다시 묻지 않기",
        send: "전송",
        reset_to_original: "원본으로 되돌리기",
        compare_with_original: "원본과 비교",
        send_to_compare: "요청을 전송하면 캡처된 결과를 원본과 비교합니다.",
        name_hint: "이름",
        value_hint: "값",
        add_header: "헤더 추가",
        partial_body_preview_suffix: "\n[부분 본문 미리보기]",
        preview_updating: "[미리보기 업데이트 중]",
    }
    ko_tooltips = {
        headers_hint: "쿠키는 Cookie: name=value; name2=value2 형식으로 쓰며 hop-by-hop 헤더는 제거되고 Content-Length는 자동 계산됩니다.",
    }
}

/// Header line referencing the original captured record id.
#[must_use]
pub(crate) fn original_label(lang: Language, original_id: u64) -> String {
    match lang {
        Language::English => format!("Original #{original_id}"),
        Language::Korean => format!("원본 #{original_id}"),
    }
}

/// Summary shown when a request body is too large or binary to edit inline.
#[must_use]
pub(crate) fn body_summary(
    lang: Language,
    size: usize,
    editable_limit: usize,
    source: Option<&str>,
) -> String {
    match (lang, source) {
        (Language::English, Some(file)) if size > editable_limit => format!(
            "{size} bytes loaded from {file}. Too large to edit here; it is sent as loaded."
        ),
        (Language::English, None) if size > editable_limit => {
            format!("{size} bytes. Too large to edit here; it is sent as loaded.")
        }
        (Language::Korean, Some(file)) if size > editable_limit => format!(
            "{file}에서 불러온 {size}바이트입니다. 여기서 편집하기엔 크기가 커서 불러온 그대로 전송됩니다."
        ),
        (Language::Korean, None) if size > editable_limit => {
            format!("{size}바이트입니다. 여기서 편집하기엔 크기가 커서 불러온 그대로 전송됩니다.")
        }
        (Language::English, Some(file)) => {
            format!(
                "Binary body: {size} bytes loaded from {file}. Load a replacement file to edit."
            )
        }
        (Language::English, None) => {
            format!("Binary body: {size} bytes. Load a replacement file to edit.")
        }
        (Language::Korean, Some(file)) => {
            format!(
                "바이너리 본문: {file}에서 불러온 {size}바이트. 변경하려면 대체 파일을 불러오세요."
            )
        }
        (Language::Korean, None) => {
            format!("바이너리 본문: {size}바이트. 변경하려면 대체 파일을 불러오세요.")
        }
    }
}
