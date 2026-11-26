//! Wrapping and continuation indentation for the read-only body view.

use std::sync::Arc;

use eframe::egui::{Galley, text::LayoutJob};

use crate::ui::style::tokens::Layout;

/// Lays out the wrapped job, retrying with token splitting only when
/// the font engine would otherwise discard its leading indentation.
pub(super) fn wrapped_galley(
    job: &LayoutJob,
    wrap_width: f32,
    mut layout: impl FnMut(LayoutJob) -> Arc<Galley>,
) -> Arc<Galley> {
    let mut wrapped = job.clone();
    wrapped.wrap.max_width = wrap_width;
    let has_indent = clamp_leading_space(&mut wrapped, wrap_width);
    let initial = layout(wrapped.clone());
    if has_indent && dropped_leading_indent(&initial) {
        wrapped.wrap.break_anywhere = true;
        layout(wrapped)
    } else {
        initial
    }
}

/// Bounds visual continuation indents on narrow viewports so long prefixes
/// never squeeze continuation lines into unreadable narrow columns.
/// Returns `true` when at least one section keeps a non-zero visual indent.
fn clamp_leading_space(job: &mut LayoutJob, wrap_width: f32) -> bool {
    let max_indent = if wrap_width.is_finite() {
        Some((wrap_width * 0.5).min((wrap_width - Layout::BODY_MIN_WRAP_TEXT).max(0.0)))
    } else {
        None
    };
    let mut has_indent = false;
    for section in &mut job.sections {
        if let Some(limit) = max_indent
            && section.leading_space > limit
        {
            section.leading_space = limit;
        }
        if section.leading_space > 0.0 {
            has_indent = true;
        }
    }
    has_indent
}

/// Detects epaint's fallback that inserts a non-final zero-height empty row and
/// resets `leading_space` to zero when a single unbreakable token exceeds the
/// space remaining after the indent. Normal empty rows at a trailing `\n` are
/// final and have `row.size().y > 0.0`.
fn dropped_leading_indent(galley: &Galley) -> bool {
    let check_len = galley.rows.len().saturating_sub(1);
    galley
        .rows
        .iter()
        .take(check_len)
        .any(|row| !row.ends_with_newline && row.glyphs.is_empty() && row.size.y == 0.0)
}
