//! Following the newest request: the list stays at the bottom until the
//! user scrolls up or picks a row, then offers `↓ N new requests` to jump
//! back and follow again.

use eframe::egui;
use http_inspector_exchange::RecordId;

use crate::ui::style::controls::primary_button;
use crate::ui::style::tokens::Space;

/// What the user did to the list's scroll position this frame.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Scroll {
    None,
    Up,
    Down,
}

impl Scroll {
    /// Wheel or touchpad scrolling over `area` this frame.
    pub(super) fn of_pointer(ui: &egui::Ui, area: egui::Rect) -> Self {
        if !ui.rect_contains_pointer(area) {
            return Self::None;
        }
        // The list's scroll area consumes the smoothed delta, so read the
        // raw wheel events. Positive y moves the content down: scrolling up.
        let delta: f32 = ui.input(|input| {
            input
                .raw
                .events
                .iter()
                .filter_map(|event| match event {
                    egui::Event::MouseWheel { delta, .. } => Some(delta.y),
                    _ => None,
                })
                .sum()
        });
        if delta > 0.0 {
            Self::Up
        } else if delta < 0.0 {
            Self::Down
        } else {
            Self::None
        }
    }
}

/// What the list showed in one frame, in arrival order.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct Frame {
    /// Newest row in the list.
    pub(super) newest: Option<RecordId>,
    /// Whether the newest row was on screen.
    pub(super) newest_visible: bool,
    pub(super) scroll: Scroll,
}

/// How many rows arrived after the newest one the user saw.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum NewRows {
    Count(usize),
    /// The last seen row left the list, through eviction or a filter.
    Unknown,
}

/// Whether the list follows the newest request.
#[derive(Debug)]
pub(super) struct Follow {
    following: bool,
    /// Newest row the user has seen while following or since resuming.
    seen: Option<RecordId>,
    /// Scroll to the newest row in the next frame.
    jump: bool,
    /// Direction of the user's latest scrolling since the last pause or
    /// resume. Smooth scrolling reaches the bottom frames after the wheel
    /// event, so the direction is kept until then.
    direction: Scroll,
    previous: Option<Frame>,
}

impl Follow {
    pub(super) const fn new() -> Self {
        Self {
            following: true,
            seen: None,
            jump: false,
            direction: Scroll::None,
            previous: None,
        }
    }

    pub(super) const fn following(&self) -> bool {
        self.following
    }

    /// Stops following, for example when a row is selected.
    pub(super) const fn pause(&mut self) {
        self.following = false;
        self.direction = Scroll::None;
    }

    /// Follows again from the newest row, scrolling to it next frame.
    pub(super) const fn resume(&mut self) {
        self.following = true;
        self.jump = true;
        self.direction = Scroll::None;
    }

    /// Whether the list must scroll to the newest row in this frame.
    pub(super) const fn take_jump(&mut self) -> bool {
        std::mem::replace(&mut self.jump, false)
    }

    /// Updates the state after a frame. Scrolling up, or dragging the
    /// scroll bar away from the newest row, pauses; scrolling back down to
    /// the newest row resumes.
    pub(super) fn observe(&mut self, frame: Frame) {
        let dragged_away = self.previous.is_some_and(|previous| {
            previous.newest == frame.newest && previous.newest_visible && !frame.newest_visible
        });
        if frame.scroll != Scroll::None {
            self.direction = frame.scroll;
        }
        if self.following {
            if frame.scroll == Scroll::Up || (dragged_away && frame.scroll != Scroll::Down) {
                self.following = false;
            } else {
                self.seen = frame.newest;
            }
        } else if frame.newest_visible && self.direction == Scroll::Down {
            self.following = true;
            self.direction = Scroll::None;
            self.seen = frame.newest;
        }
        self.previous = Some(frame);
    }

    /// Rows after the last seen one, in `rows` arrival order.
    pub(super) fn new_rows(&self, rows: impl DoubleEndedIterator<Item = RecordId>) -> NewRows {
        let Some(seen) = self.seen else {
            return NewRows::Unknown;
        };
        rows.rev()
            .position(|id| id == seen)
            .map_or(NewRows::Unknown, NewRows::Count)
    }
}

/// Text of the jump button, or `None` when there is nothing to jump to.
pub(super) fn jump_label(new_rows: NewRows, newest_visible: bool) -> Option<String> {
    match new_rows {
        NewRows::Count(0) => None,
        NewRows::Count(1) => Some("↓ 1 new request".to_owned()),
        NewRows::Count(count) => Some(format!("↓ {count} new requests")),
        NewRows::Unknown if newest_visible => None,
        NewRows::Unknown => Some("↓ Jump to newest".to_owned()),
    }
}

/// Floats the jump button at the bottom center of `area`. Returns whether
/// it was clicked.
pub(super) fn jump_button(ui: &egui::Ui, area: egui::Rect, label: &str) -> bool {
    let anchor = egui::pos2(area.center().x, area.bottom() - Space::MD);
    egui::Area::new(ui.scope_id().with("follow-newest"))
        .order(egui::Order::Middle)
        .pivot(egui::Align2::CENTER_BOTTOM)
        .fixed_pos(anchor)
        .constrain_to(area)
        .show(ui.ctx(), |ui| {
            ui.add(primary_button(ui, label).wrap_mode(egui::TextWrapMode::Extend))
                .on_hover_text("Scroll to the newest request and keep following new ones.")
                .clicked()
        })
        .inner
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(count: u64) -> Vec<RecordId> {
        (0..count).map(RecordId::new).collect()
    }

    fn frame(newest: Option<RecordId>, visible: bool, scroll: Scroll) -> Frame {
        Frame {
            newest,
            newest_visible: visible,
            scroll,
        }
    }

    #[test]
    fn scrolling_up_pauses_and_counts_new_rows() {
        let rows = ids(5);
        let mut follow = Follow::new();
        follow.observe(frame(rows.get(2).copied(), true, Scroll::None));
        follow.observe(frame(rows.get(2).copied(), true, Scroll::Up));
        assert!(!follow.following());
        assert_eq!(follow.new_rows(rows.iter().copied()), NewRows::Count(2));
        assert_eq!(
            jump_label(NewRows::Count(2), false).as_deref(),
            Some("↓ 2 new requests")
        );
        assert_eq!(jump_label(NewRows::Count(0), false), None);
    }

    #[test]
    fn dragging_the_scroll_bar_away_pauses_but_new_rows_do_not() {
        let rows = ids(3);
        let mut follow = Follow::new();
        follow.observe(frame(rows.first().copied(), true, Scroll::None));
        // A new row that is not on screen yet does not pause.
        follow.observe(frame(rows.get(1).copied(), false, Scroll::None));
        assert!(follow.following());
        follow.observe(frame(rows.get(1).copied(), true, Scroll::None));
        follow.observe(frame(rows.get(1).copied(), false, Scroll::None));
        assert!(!follow.following());
    }

    #[test]
    fn jump_or_scrolling_back_down_resumes() {
        let rows = ids(4);
        let mut follow = Follow::new();
        follow.pause();
        follow.resume();
        assert!(follow.following() && follow.take_jump() && !follow.take_jump());

        follow.pause();
        // Already at the bottom after a selection: no scrolling, no resume.
        follow.observe(frame(rows.get(3).copied(), true, Scroll::None));
        assert!(!follow.following());
        follow.observe(frame(rows.get(3).copied(), false, Scroll::Down));
        assert!(!follow.following());
        // Smooth scrolling reaches the bottom a few frames after the wheel.
        follow.observe(frame(rows.get(3).copied(), true, Scroll::None));
        assert!(follow.following());
        assert_eq!(follow.new_rows(rows.iter().copied()), NewRows::Count(0));
    }

    #[test]
    fn an_evicted_seen_row_offers_a_plain_jump() {
        let rows = ids(4);
        let mut follow = Follow::new();
        follow.observe(frame(rows.first().copied(), true, Scroll::Up));
        assert_eq!(
            follow.new_rows(rows.iter().skip(1).copied()),
            NewRows::Unknown
        );
        assert_eq!(
            jump_label(NewRows::Unknown, false).as_deref(),
            Some("↓ Jump to newest")
        );
        assert_eq!(jump_label(NewRows::Unknown, true), None);
    }
}
