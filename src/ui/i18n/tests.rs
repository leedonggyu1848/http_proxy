//! Unit tests for language detection, preference persistence, catalog
//! completeness, and UI copy policy (`S1`: single-sentence tooltips).

use std::ffi::OsString;
use std::fs;
use std::io;

use super::{
    LANGUAGE_FILE, Language, LanguagePreference, detail, detect_system_language, dialogs, domain,
    language_from_locale, load_from, save_to, toolbar,
};

const LANGUAGES: [Language; 2] = [Language::English, Language::Korean];

fn env_from<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl FnMut(&str) -> Option<OsString> + 'a {
    move |name| {
        pairs
            .iter()
            .find(|(key, _)| *key == name)
            .map(|(_, value)| OsString::from(value))
    }
}

fn is_closing_punctuation(ch: char) -> bool {
    matches!(ch, '"' | '\'' | '”' | '’' | ')' | ']' | '»' | '」' | '』')
}

/// Checks the `S1` tooltip policy from `.agents/skills/ui-unslop/SKILL.md`:
/// a tooltip must be a single line and at most one sentence.
fn is_single_sentence(text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.is_empty() || trimmed.contains('\n') || trimmed.contains('\r') {
        return false;
    }
    let chars: Vec<(usize, char)> = trimmed.char_indices().collect();
    for (pos, &(byte_idx, ch)) in chars.iter().enumerate() {
        if !matches!(ch, '.' | '!' | '?') {
            continue;
        }
        if ch == '.' && is_abbreviation_before(trimmed, byte_idx) {
            continue;
        }
        let mut after = pos + 1;
        while let Some(&(_, closing)) = chars.get(after) {
            if is_closing_punctuation(closing) {
                after += 1;
            } else {
                break;
            }
        }
        let Some(&(_, next)) = chars.get(after) else {
            continue;
        };
        if !next.is_whitespace() {
            // Part of a filename, extension, IP address, or HTTP version (e.g. `.har`, `1.1`).
            continue;
        }
        // Any remaining non-whitespace characters after this terminal punctuation
        // (and optional closing quote/bracket) indicate a second sentence.
        if chars
            .iter()
            .skip(after + 1)
            .any(|&(_, trailing)| !trailing.is_whitespace())
        {
            return false;
        }
    }
    true
}

fn is_abbreviation_before(text: &str, dot_index: usize) -> bool {
    let prefix = &text[..dot_index];
    prefix.ends_with("e.g") || prefix.ends_with("i.e")
}

/// Collects `(catalog_name, field_name, value)` for every static catalog entry.
fn all_static_entries(lang: Language) -> Vec<(&'static str, &'static str, &'static str)> {
    let mut all = Vec::new();
    let mut push = |catalog: &'static str, entries: Vec<(&'static str, &'static str)>| {
        for (field, value) in entries {
            all.push((catalog, field, value));
        }
    };

    push("shared", lang.shared().entries());
    push("shared.tooltips", lang.shared().tooltips.entries());
    push("toolbar", lang.toolbar().entries());
    push("toolbar.tooltips", lang.toolbar().tooltips.entries());
    push("requests", lang.requests().entries());
    push("requests.tooltips", lang.requests().tooltips.entries());
    push("detail", lang.detail().entries());
    push("detail.tooltips", lang.detail().tooltips.entries());
    push("process", lang.process().entries());
    push("map", lang.map().entries());
    push("map.tooltips", lang.map().tooltips.entries());
    push("dialogs::browser", lang.browser().entries());
    push(
        "dialogs::browser.tooltips",
        lang.browser().tooltips.entries(),
    );
    push("dialogs::ca", lang.ca().entries());
    push("dialogs::ca.tooltips", lang.ca().tooltips.entries());
    push(
        "dialogs::ca",
        vec![("keychain_help", dialogs::ca::keychain_help(lang))],
    );
    push("dialogs::copy_curl", lang.copy_curl().entries());
    push(
        "dialogs::copy_curl.tooltips",
        lang.copy_curl().tooltips.entries(),
    );
    push("dialogs::har", lang.har().entries());
    push("dialogs::har.tooltips", lang.har().tooltips.entries());
    push("dialogs::scope_help", lang.scope_help().entries());
    push(
        "dialogs::scope_help.tooltips",
        lang.scope_help().tooltips.entries(),
    );
    push("dialogs::save", lang.save().entries());
    push("dialogs::save.tooltips", lang.save().tooltips.entries());
    push("dialogs::bulk_save", lang.bulk_save().entries());
    push(
        "dialogs::bulk_save.tooltips",
        lang.bulk_save().tooltips.entries(),
    );
    push("dialogs::body_saving", lang.body_saving().entries());
    push(
        "dialogs::body_saving.tooltips",
        lang.body_saving().tooltips.entries(),
    );
    push("dialogs::range_merge", lang.range_merge().entries());
    push(
        "dialogs::range_merge.tooltips",
        lang.range_merge().tooltips.entries(),
    );
    push("dialogs::replay", lang.replay().entries());
    push("dialogs::replay.tooltips", lang.replay().tooltips.entries());
    push("dialogs::hls", lang.hls().entries());

    all
}

/// Collects `(catalog_name, field_name, value)` for every static tooltip field.
fn all_static_tooltips(lang: Language) -> Vec<(&'static str, &'static str, &'static str)> {
    let mut all = Vec::new();
    let mut push = |catalog: &'static str, entries: Vec<(&'static str, &'static str)>| {
        for (field, value) in entries {
            all.push((catalog, field, value));
        }
    };

    push("shared.tooltips", lang.shared().tooltips.entries());
    push("toolbar.tooltips", lang.toolbar().tooltips.entries());
    push("requests.tooltips", lang.requests().tooltips.entries());
    push("detail.tooltips", lang.detail().tooltips.entries());
    push("process", lang.process().entries());
    push("map.tooltips", lang.map().tooltips.entries());
    push(
        "dialogs::browser.tooltips",
        lang.browser().tooltips.entries(),
    );
    push("dialogs::ca.tooltips", lang.ca().tooltips.entries());
    push(
        "dialogs::copy_curl.tooltips",
        lang.copy_curl().tooltips.entries(),
    );
    push("dialogs::har.tooltips", lang.har().tooltips.entries());
    push(
        "dialogs::scope_help.tooltips",
        lang.scope_help().tooltips.entries(),
    );
    push("dialogs::save.tooltips", lang.save().tooltips.entries());
    push(
        "dialogs::bulk_save.tooltips",
        lang.bulk_save().tooltips.entries(),
    );
    push(
        "dialogs::body_saving.tooltips",
        lang.body_saving().tooltips.entries(),
    );
    push(
        "dialogs::range_merge.tooltips",
        lang.range_merge().tooltips.entries(),
    );
    push("dialogs::replay.tooltips", lang.replay().tooltips.entries());

    all
}

#[test]
fn all_catalog_strings_are_non_empty_in_every_language() {
    for lang in LANGUAGES {
        let entries = all_static_entries(lang);
        assert!(
            !entries.is_empty(),
            "catalog entry list must not be empty for {lang:?}"
        );
        for (catalog, field, value) in entries {
            assert!(
                !value.trim().is_empty(),
                "{catalog}.{field} resolved to empty text for {lang:?}"
            );
        }
    }
}

#[test]
fn all_static_tooltips_follow_s1_single_sentence_rule() {
    for lang in LANGUAGES {
        for (catalog, field, value) in all_static_tooltips(lang) {
            assert!(
                is_single_sentence(value),
                "S1 violation: tooltip {catalog}.{field} for {lang:?} must be at most one sentence: {value:?}"
            );
        }
    }
}

fn all_curl_unavailable_variants() -> Vec<http_inspector_interop::curl::CurlUnavailable> {
    use http_inspector_interop::curl::CurlUnavailable;

    let variants = vec![
        CurlUnavailable::Tunnel,
        CurlUnavailable::TruncatedUrl,
        CurlUnavailable::UnsupportedUrl,
        CurlUnavailable::OmittedHeaders,
        CurlUnavailable::UnreproducibleHeader("X-Raw".to_owned()),
        CurlUnavailable::BodyInProgress,
        CurlUnavailable::BodyIncomplete,
        CurlUnavailable::BodyTruncated {
            retained: 8192,
            observed: 65536,
        },
        CurlUnavailable::BodyNotCaptured,
    ];
    for variant in &variants {
        // Exhaustive match so new `CurlUnavailable` variants must be added to this test.
        match variant {
            CurlUnavailable::Tunnel
            | CurlUnavailable::TruncatedUrl
            | CurlUnavailable::UnsupportedUrl
            | CurlUnavailable::OmittedHeaders
            | CurlUnavailable::UnreproducibleHeader(_)
            | CurlUnavailable::BodyInProgress
            | CurlUnavailable::BodyIncomplete
            | CurlUnavailable::BodyTruncated { .. }
            | CurlUnavailable::BodyNotCaptured => {}
        }
    }
    variants
}

fn all_matcher_error_variants() -> Vec<http_inspector_analysis::search::MatcherError> {
    use http_inspector_analysis::search::MatcherError;

    let variants = vec![
        MatcherError::Empty,
        MatcherError::Regex("unclosed group".to_owned()),
    ];
    for variant in &variants {
        match variant {
            MatcherError::Empty | MatcherError::Regex(_) => {}
        }
    }
    variants
}

const FULL_LOG_RUNTIME_SAMPLES: &[&str] = &[
    "Session logging stopped because its writer disconnected; the log is incomplete",
    "The session journal queue is unavailable; the log is incomplete",
    "Session logging failed; the log is incomplete: disk full",
    "Session logging is busy; retry saving the log shortly",
    "The session journal snapshot ended early",
];

const HLS_SEGMENT_FAILURE_SAMPLES: &[&str] = &[
    "HTTP 403 Forbidden on segment #1 (seg001.ts)",
    "Too many redirects on segment #2 (seg002.ts)",
    "Invalid redirect on segment #3 (seg003.ts): missing Location",
    "Network error on segment #4 (seg004.ts): connection reset",
    "Decoding failed on segment #5 (seg005.ts): invalid gzip",
    "Cached segment #6 (seg006.ts) could not be read: permission denied",
    "TLS setup failed: certificate error",
    "File write failed: disk full",
];

const FILTER_DIAGNOSTIC_SAMPLES: &[&str] = &[
    "Unclosed quote",
    "Empty search term",
    "Reserved filter key `body`",
    "Unknown filter key `foo`",
    "Missing value for `status`",
    "Invalid status value `abc`",
];

fn dynamic_tooltip_samples(lang: Language) -> Vec<(&'static str, String)> {
    let mut samples = vec![
        ("detail::focus_filter", detail::focus_filter(lang, "Ctrl+F")),
        (
            "detail::background_search_unavailable",
            detail::background_search_unavailable(lang, "worker stopped"),
        ),
        (
            "detail::search_coverage_detail",
            detail::search_coverage_detail(lang, 2, 1, 1, 256).unwrap_or_default(),
        ),
        (
            "toolbar::older_removed_hint",
            toolbar::older_removed_hint(lang, 5000),
        ),
        (
            "toolbar::url_map_limit_hint",
            toolbar::url_map_limit_hint(lang, 2000),
        ),
        (
            "toolbar::log_body_limit_hint",
            toolbar::log_body_limit_hint(lang, "64 MiB"),
        ),
        (
            "dialogs::copy_curl::redacted_placeholders_hint",
            dialogs::copy_curl::redacted_placeholders_hint(lang),
        ),
        (
            "dialogs::body_saving::init_open_failed",
            dialogs::body_saving::init_open_failed(lang, "permission denied"),
        ),
        (
            "domain::full_log_error",
            domain::full_log_error(lang, &io::Error::other("disk full")),
        ),
    ];

    for variant in &all_curl_unavailable_variants() {
        samples.push((
            "domain::curl_unavailable",
            domain::curl_unavailable(lang, variant),
        ));
    }
    for variant in &all_matcher_error_variants() {
        samples.push((
            "domain::matcher_error",
            domain::matcher_error(lang, variant),
        ));
    }
    for raw in FULL_LOG_RUNTIME_SAMPLES {
        samples.push((
            "domain::localize_full_log_runtime_error",
            domain::localize_full_log_runtime_error(lang, raw),
        ));
    }
    for raw in HLS_SEGMENT_FAILURE_SAMPLES {
        samples.push((
            "domain::localize_hls_segment_failure",
            domain::localize_hls_segment_failure(lang, raw),
        ));
    }
    for raw in FILTER_DIAGNOSTIC_SAMPLES {
        samples.push((
            "domain::localize_filter_diagnostic",
            domain::localize_filter_diagnostic(lang, raw),
        ));
    }
    samples
}

#[test]
fn dynamic_tooltips_follow_s1_single_sentence_rule() {
    for lang in LANGUAGES {
        for (name, value) in dynamic_tooltip_samples(lang) {
            assert!(
                !value.trim().is_empty() && is_single_sentence(&value),
                "S1 violation: dynamic tooltip {name} for {lang:?} must be a non-empty single sentence: {value:?}"
            );
        }
    }
}

#[test]
fn single_sentence_detector_accepts_one_sentence_and_rejects_multiple() {
    let valid_cases = [
        "Save the listed requests as a HAR 1.2 file, without bodies.",
        "Loopback port to listen on; 0 picks a free port.",
        "Save bodies into a new folder with a manifest.json.",
        "What is captured?",
        "무엇이 캡처되나요?",
        "디코딩된 본문을 저장 폴더에 저장합니다.",
        "Use e.g. 127.0.0.1 for local HTTP/1.1 tests.",
        "Click \"Save.\"",
        "Save bodies (e.g. manifest.json) to disk.",
        "Use \"manifest.json\" here.",
    ];
    for case in valid_cases {
        assert!(is_single_sentence(case), "expected valid: {case:?}");
    }

    let invalid_cases = [
        "",
        "   ",
        "First sentence. Second sentence.",
        "첫 번째 문장입니다. 두 번째 문장입니다.",
        "Question? Answer.",
        "Warning! Do not proceed.",
        "Click \"Save.\" Then reopen.",
        "(First sentence.) Second sentence.",
        "\"복사했습니다.\" 다시 시도하세요.",
        "Is it \"ready?\" Yes.",
        "Watch out!) Now stop.",
        "Line one\nLine two",
    ];
    for case in invalid_cases {
        assert!(!is_single_sentence(case), "expected invalid: {case:?}");
    }
}

#[test]
fn parses_known_preferences_and_rejects_unknown_values() {
    assert_eq!(
        LanguagePreference::parse("system"),
        Some(LanguagePreference::System)
    );
    assert_eq!(
        LanguagePreference::parse("  EN \n"),
        Some(LanguagePreference::English)
    );
    assert_eq!(
        LanguagePreference::parse("english"),
        Some(LanguagePreference::English)
    );
    assert_eq!(
        LanguagePreference::parse("ko\n"),
        Some(LanguagePreference::Korean)
    );
    assert_eq!(
        LanguagePreference::parse("KO"),
        Some(LanguagePreference::Korean)
    );
    assert_eq!(
        LanguagePreference::parse("korean"),
        Some(LanguagePreference::Korean)
    );
    assert_eq!(LanguagePreference::parse(""), None);
    assert_eq!(LanguagePreference::parse("ja"), None);
    assert_eq!(LanguagePreference::parse("ko_KR"), None);
}

#[test]
fn saves_and_loads_language_preference_and_falls_back_on_invalid_file() -> io::Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("nested").join(LANGUAGE_FILE);

    assert_eq!(load_from(&path), LanguagePreference::System);

    for preference in LanguagePreference::ALL {
        save_to(&path, preference)?;
        assert_eq!(load_from(&path), preference);
    }

    fs::write(&path, "unexpected-value\n")?;
    assert_eq!(load_from(&path), LanguagePreference::System);

    fs::write(&path, "")?;
    assert_eq!(load_from(&path), LanguagePreference::System);
    Ok(())
}

#[test]
fn detects_system_language_following_posix_precedence() {
    assert_eq!(
        detect_system_language(env_from(&[
            ("LC_ALL", "ko_KR.UTF-8"),
            ("LC_MESSAGES", "en_US.UTF-8"),
            ("LANG", "en_US.UTF-8"),
        ])),
        Language::Korean
    );
    assert_eq!(
        detect_system_language(env_from(&[
            ("LC_ALL", "C"),
            ("LC_MESSAGES", "ko_KR.UTF-8"),
            ("LANG", "ko_KR.UTF-8"),
        ])),
        Language::English
    );
    assert_eq!(
        detect_system_language(env_from(&[
            ("LC_ALL", "   "),
            ("LC_MESSAGES", "ko-KR"),
            ("LANG", "en_US.UTF-8"),
        ])),
        Language::Korean
    );
    assert_eq!(
        detect_system_language(env_from(&[
            ("LC_ALL", ""),
            ("LC_MESSAGES", ""),
            ("LANG", "ko"),
        ])),
        Language::Korean
    );
    assert_eq!(detect_system_language(env_from(&[])), Language::English);
}

#[test]
fn parses_locale_variants_without_false_positives() {
    for korean in [
        "ko",
        "KO",
        "ko_KR",
        "ko-KR",
        "ko_KR.UTF-8",
        "ko_KR.utf8@modifier",
    ] {
        assert_eq!(language_from_locale(korean), Language::Korean, "{korean}");
    }
    for english in [
        "en_US.UTF-8",
        "C",
        "POSIX",
        "ja_JP.UTF-8",
        "kok_IN.UTF-8",
        "koi8-r",
        "",
    ] {
        assert_eq!(
            language_from_locale(english),
            Language::English,
            "{english}"
        );
    }
}

#[test]
fn language_menu_labels_preserve_native_endonyms() {
    for active in LANGUAGES {
        assert_eq!(LanguagePreference::English.menu_label(active), "English");
        assert_eq!(LanguagePreference::Korean.menu_label(active), "한국어");
    }
    assert_eq!(
        LanguagePreference::System.menu_label(Language::English),
        "System"
    );
    assert_eq!(
        LanguagePreference::System.menu_label(Language::Korean),
        "시스템"
    );
}
