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
        let Some(&(_, next)) = chars.get(pos + 1) else {
            continue;
        };
        if !next.is_whitespace() {
            // Part of a filename, extension, IP address, or HTTP version (e.g. `.har`, `1.1`).
            continue;
        }
        if ch == '.' && is_abbreviation_before(trimmed, byte_idx) {
            continue;
        }
        // Any remaining non-whitespace characters after this terminal punctuation
        // indicate a second sentence.
        if chars
            .iter()
            .skip(pos + 1)
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

#[test]
fn dynamic_tooltips_follow_s1_single_sentence_rule() {
    for lang in LANGUAGES {
        let samples = [
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
                "domain::curl_unavailable(Tunnel)",
                domain::curl_unavailable(
                    lang,
                    &http_inspector_interop::curl::CurlUnavailable::Tunnel,
                ),
            ),
        ];
        for (name, value) in samples {
            assert!(
                !value.trim().is_empty() && is_single_sentence(&value),
                "S1 violation: dynamic tooltip {name} for {lang:?} must be a non-empty single sentence: {value:?}"
            );
        }
    }
}

#[test]
fn single_sentence_detector_accepts_one_sentence_and_rejects_multiple() {
    assert!(is_single_sentence(
        "Save the listed requests as a HAR 1.2 file, without bodies."
    ));
    assert!(is_single_sentence(
        "Loopback port to listen on; 0 picks a free port."
    ));
    assert!(is_single_sentence(
        "Save bodies into a new folder with a manifest.json."
    ));
    assert!(is_single_sentence("What is captured?"));
    assert!(is_single_sentence("무엇이 캡처되나요?"));
    assert!(is_single_sentence(
        "디코딩된 본문을 저장 폴더에 저장합니다."
    ));
    assert!(is_single_sentence(
        "Use e.g. 127.0.0.1 for local HTTP/1.1 tests."
    ));

    assert!(!is_single_sentence(""));
    assert!(!is_single_sentence("   "));
    assert!(!is_single_sentence("First sentence. Second sentence."));
    assert!(!is_single_sentence(
        "첫 번째 문장입니다. 두 번째 문장입니다."
    ));
    assert!(!is_single_sentence("Question? Answer."));
    assert!(!is_single_sentence("Warning! Do not proceed."));
    assert!(!is_single_sentence("Line one\nLine two"));
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
