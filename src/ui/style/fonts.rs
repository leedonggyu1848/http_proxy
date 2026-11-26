//! Font chain for the UI.
//!
//! egui draws only fonts registered in [`FontDefinitions`]; it never consults
//! fontconfig or `CoreText`. Every family therefore uses the chain
//! `egui defaults → embedded Korean font → optional system CJK font`, so
//! Hangul and ASCII render without any installed font and a system font only
//! adds Hanja and kana.

use std::io;
use std::path::{Path, PathBuf};

use eframe::egui::{self, FontData, FontDefinitions, FontFamily};
use skrifa::MetadataProvider as _;
use skrifa::raw::{FileRef, FontRef, ReadError};
use skrifa::string::StringId;

/// Noto Sans KR 2.004 Regular subset to ASCII, Hangul and common punctuation.
/// See `assets/fonts/README.md` for its source and how to regenerate it.
const EMBEDDED_KOREAN: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/assets/fonts/NotoSansKR-Regular-subset.otf"
));
const EMBEDDED_KOREAN_NAME: &str = "noto-sans-kr-subset";
const SYSTEM_CJK_NAME: &str = "system-cjk";

/// System fonts tried in order to supplement Hanja and kana.
const SYSTEM_CJK_CANDIDATES: [&str; 6] = [
    "/System/Library/Fonts/AppleSDGothicNeo.ttc",
    "/usr/share/fonts/truetype/nanum/NanumGothicCoding.ttf",
    "/usr/share/fonts/truetype/nanum/NanumGothic.ttf",
    "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
    "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
    "/usr/share/fonts/google-noto-cjk/NotoSansCJK-Regular.ttc",
];

pub(crate) fn configure_fonts(context: &egui::Context) {
    let candidates = SYSTEM_CJK_CANDIDATES.map(Path::new);
    context.set_fonts(font_definitions(&candidates));
}

/// Builds egui's default fonts followed by the embedded Korean font and, when
/// one of `system_candidates` parses, a system CJK font.
fn font_definitions(system_candidates: &[&Path]) -> FontDefinitions {
    let mut fonts = FontDefinitions::default();
    append_fallback(
        &mut fonts,
        EMBEDDED_KOREAN_NAME,
        FontData::from_static(EMBEDDED_KOREAN),
    );
    tracing::debug!(
        font = EMBEDDED_KOREAN_NAME,
        "registered embedded Korean font"
    );
    append_system_cjk(&mut fonts, system_candidates);
    fonts
}

/// Appends the first usable system CJK font after the embedded font, or logs
/// that only the embedded font is available.
fn append_system_cjk(fonts: &mut FontDefinitions, system_candidates: &[&Path]) {
    let Some(system) = find_system_cjk(system_candidates) else {
        tracing::info!(
            "no usable system CJK font found; Hangul uses the embedded font, Hanja and kana may not render"
        );
        return;
    };
    tracing::debug!(
        path = %system.path.display(),
        face_index = system.face_index,
        "registered system CJK font after the embedded Korean font"
    );
    let mut data = FontData::from_owned(system.data);
    data.index = system.face_index;
    append_fallback(fonts, SYSTEM_CJK_NAME, data);
}

fn append_fallback(fonts: &mut FontDefinitions, name: &str, data: FontData) {
    fonts.font_data.insert(name.to_owned(), data.into());
    for family in [FontFamily::Proportional, FontFamily::Monospace] {
        fonts
            .families
            .entry(family)
            .or_default()
            .push(name.to_owned());
    }
}

/// A system font file that parsed, with the face egui should use.
struct SystemFont {
    path: PathBuf,
    data: Vec<u8>,
    face_index: u32,
}

/// Returns the first candidate that can be read and parsed, logging the
/// candidates that exist but are unusable.
fn find_system_cjk(candidates: &[&Path]) -> Option<SystemFont> {
    candidates.iter().find_map(|path| {
        let data = match std::fs::read(path) {
            Ok(data) => data,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return None,
            Err(error) => {
                tracing::warn!(path = %path.display(), %error, "skipping unreadable system CJK font");
                return None;
            }
        };
        match select_face(&data) {
            Ok(face_index) => Some(SystemFont {
                path: path.to_path_buf(),
                data,
                face_index,
            }),
            Err(error) => {
                tracing::warn!(path = %path.display(), %error, "skipping system CJK font that does not parse");
                None
            }
        }
    })
}

#[derive(Debug, thiserror::Error)]
enum FontRejection {
    #[error("not a TTF/OTF/TTC font: {0}")]
    Parse(#[from] ReadError),
    #[error("font has no character map")]
    NoCharmap,
    #[error("font collection has no usable face")]
    NoUsableFace,
}

/// Validates the file the same way epaint parses it and picks a face index:
/// `0` for a single font, the Korean face of a collection when its family
/// name says so, otherwise the first usable face.
fn select_face(data: &[u8]) -> Result<u32, FontRejection> {
    match FileRef::new(data)? {
        FileRef::Font(font) => ensure_usable(&font).map(|()| 0),
        FileRef::Collection(collection) => {
            let usable = (0..collection.len()).filter_map(|index| {
                let font = collection.get(index).ok()?;
                ensure_usable(&font).ok()?;
                Some((index, is_korean_face(&font)))
            });
            let mut first_usable = None;
            for (index, korean) in usable {
                if korean {
                    return Ok(index);
                }
                first_usable.get_or_insert(index);
            }
            first_usable.ok_or(FontRejection::NoUsableFace)
        }
    }
}

fn ensure_usable(font: &FontRef<'_>) -> Result<(), FontRejection> {
    if font.charmap().has_map() {
        Ok(())
    } else {
        Err(FontRejection::NoCharmap)
    }
}

fn is_korean_face(font: &FontRef<'_>) -> bool {
    font.localized_strings(StringId::FAMILY_NAME)
        .english_or_first()
        .is_some_and(|name| is_korean_family_name(&name.to_string()))
}

/// Matches collection face names such as `Noto Sans CJK KR`.
fn is_korean_family_name(name: &str) -> bool {
    name.split_whitespace()
        .any(|word| word == "KR" || word.eq_ignore_ascii_case("Korean"))
}

use super::tokens::TypeScale;

#[expect(
    clippy::disallowed_methods,
    reason = "the code font is defined by the typography tokens"
)]
#[cfg(test)]
pub(crate) const fn code() -> egui::FontId {
    egui::FontId::monospace(TypeScale::CODE)
}

#[cfg(test)]
#[expect(
    clippy::disallowed_methods,
    reason = "the fixture uses the large code size"
)]
pub(crate) const fn code_large() -> egui::FontId {
    egui::FontId::monospace(TypeScale::LARGE_CODE)
}

/// Monospace status bar and secondary metadata text (`TypeScale::STATUS`).
#[expect(
    clippy::disallowed_methods,
    reason = "status typography comes from the type scale"
)]
pub(crate) const fn status() -> egui::FontId {
    egui::FontId::monospace(TypeScale::STATUS)
}

/// Sans panel and toolbar titles (`TypeScale::PANEL_TITLE`).
#[expect(
    clippy::disallowed_methods,
    reason = "title typography comes from the type scale"
)]
pub(crate) const fn panel_title() -> egui::FontId {
    egui::FontId::proportional(TypeScale::PANEL_TITLE)
}

/// Sans tabs, buttons, and other controls (`TypeScale::CONTROL`).
#[expect(
    clippy::disallowed_methods,
    reason = "control typography comes from the type scale"
)]
pub(crate) const fn control() -> egui::FontId {
    egui::FontId::proportional(TypeScale::CONTROL)
}

/// Monospace data next to controls, such as the proxy address (`TypeScale::DATA`).
#[expect(
    clippy::disallowed_methods,
    reason = "data typography comes from the type scale"
)]
pub(crate) const fn data() -> egui::FontId {
    egui::FontId::monospace(TypeScale::DATA)
}

/// The one-line error at the right end of an input (`TypeScale::STATUS`).
#[expect(
    clippy::disallowed_methods,
    reason = "input error typography comes from the type scale"
)]
pub(crate) const fn input_error() -> egui::FontId {
    egui::FontId::proportional(TypeScale::STATUS)
}

#[expect(
    clippy::disallowed_methods,
    reason = "graph typography comes from the type scale"
)]
pub(crate) const fn graph_fonts() -> (egui::FontId, egui::FontId) {
    (
        egui::FontId::proportional(TypeScale::GRAPH_LABEL),
        egui::FontId::monospace(TypeScale::GRAPH_COUNT),
    )
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use super::*;

    use egui::epaint::text::{FontId, Fonts, TextOptions};

    const KOREAN_AND_ASCII: &str = "가힣한글ㄱㅎ AZaz09:/{}";

    /// Asserts both families keep egui's defaults first and end with `tail`.
    fn assert_families_end_with(fonts: &FontDefinitions, tail: &[&str]) {
        let defaults = FontDefinitions::default();
        for family in [FontFamily::Proportional, FontFamily::Monospace] {
            let names = fonts.families.get(&family).cloned().unwrap_or_default();
            let mut expected = defaults.families.get(&family).cloned().unwrap_or_default();
            expected.extend(tail.iter().map(|name| (*name).to_owned()));
            assert_eq!(names, expected, "{family:?}");
        }
    }

    #[expect(
        clippy::disallowed_methods,
        reason = "check glyphs independently of the type scale"
    )]
    fn assert_renders_korean(fonts: FontDefinitions) {
        let mut fonts = Fonts::new(TextOptions::default(), fonts);
        for font_id in [FontId::proportional(14.0), FontId::monospace(14.0)] {
            assert!(fonts.has_glyphs(&font_id, KOREAN_AND_ASCII), "{font_id:?}");
        }
    }

    #[test]
    fn embedded_font_has_korean_and_ascii_glyphs() -> Result<(), Box<dyn Error>> {
        let font = FontRef::new(EMBEDDED_KOREAN)?;
        let charmap = font.charmap();
        let missing: Vec<char> = KOREAN_AND_ASCII
            .chars()
            .filter(|&c| charmap.map(c).is_none())
            .collect();
        assert!(missing.is_empty(), "missing glyphs: {missing:?}");
        assert_eq!(select_face(EMBEDDED_KOREAN)?, 0);
        Ok(())
    }

    #[test]
    fn unusable_system_candidates_leave_only_the_embedded_font() -> Result<(), Box<dyn Error>> {
        let directory = tempfile::tempdir()?;
        // epaint panics on the first two if they reach `set_fonts`.
        let corrupt = directory.path().join("corrupt.ttf");
        std::fs::write(&corrupt, b"not a font at all")?;
        let empty = directory.path().join("empty.ttc");
        std::fs::write(&empty, b"")?;
        // An OpenType header with no tables parses but has nothing to draw.
        let header_only = directory.path().join("header-only.otf");
        std::fs::write(&header_only, b"OTTO\0\x05truncated")?;
        let missing = directory.path().join("missing.ttf");
        let candidates = [
            corrupt.as_path(),
            empty.as_path(),
            header_only.as_path(),
            missing.as_path(),
        ];

        let fonts = font_definitions(&candidates);

        assert!(fonts.font_data.contains_key(EMBEDDED_KOREAN_NAME));
        assert!(!fonts.font_data.contains_key(SYSTEM_CJK_NAME));
        assert_families_end_with(&fonts, &[EMBEDDED_KOREAN_NAME]);
        assert_renders_korean(fonts);
        Ok(())
    }

    #[test]
    fn usable_system_candidate_follows_the_embedded_font() -> Result<(), Box<dyn Error>> {
        let directory = tempfile::tempdir()?;
        let corrupt = directory.path().join("corrupt.ttf");
        std::fs::write(&corrupt, b"\0\x01\0\0garbage")?;
        let valid = directory.path().join("valid.otf");
        std::fs::write(&valid, EMBEDDED_KOREAN)?;

        let fonts = font_definitions(&[corrupt.as_path(), valid.as_path()]);

        assert_families_end_with(&fonts, &[EMBEDDED_KOREAN_NAME, SYSTEM_CJK_NAME]);
        assert_renders_korean(fonts);
        Ok(())
    }

    #[test]
    fn korean_family_names_are_recognized() {
        assert!(is_korean_family_name("Noto Sans CJK KR"));
        assert!(is_korean_family_name("Noto Sans Mono CJK KR"));
        assert!(!is_korean_family_name("Noto Sans CJK JP"));
        assert!(!is_korean_family_name("KRONA One"));
    }
}
