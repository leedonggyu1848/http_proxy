//! Headers, Body, and Timing tabs of the detail panel: keyboard switching and an
//! accessible tab list.

use eframe::egui::{self, Key, Modifiers};

use crate::ui::i18n::Language;
use crate::ui::workbench::controls;
use crate::ui::workbench::text::ShellText;

/// Section of the selected request shown in the detail panel.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum DetailTab {
    Headers,
    Body,
    Timing,
}

/// Direction of a keyboard tab switch.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TabStep {
    Next,
    Previous,
}

impl DetailTab {
    const ALL: [Self; 3] = [Self::Headers, Self::Body, Self::Timing];

    const fn title(self, lang: Language) -> &'static str {
        match self {
            Self::Headers => ShellText::Headers.text(lang),
            Self::Body => ShellText::Body.text(lang),
            Self::Timing => ShellText::Timing.text(lang),
        }
    }

    /// Moves to the neighbouring tab, wrapping around at either end.
    fn stepped(self, step: TabStep) -> Self {
        let count = Self::ALL.len();
        let index = Self::ALL.iter().position(|&tab| tab == self).unwrap_or(0);
        let next = match step {
            TabStep::Next => (index + 1) % count,
            TabStep::Previous => (index + count - 1) % count,
        };
        Self::ALL.get(next).copied().unwrap_or(self)
    }
}

/// Maps a key press to a tab switch: `F6` and `Ctrl+PageDown` go to the next
/// tab and `Ctrl+PageUp` to the previous one.
///
/// Modifiers are compared with [`Modifiers::matches_exact`] because egui-winit
/// reports Ctrl as both `ctrl` and `command` outside macOS.
fn tab_step(key: Key, modifiers: Modifiers) -> Option<TabStep> {
    match key {
        Key::F6 if modifiers.matches_exact(Modifiers::NONE) => Some(TabStep::Next),
        Key::PageDown if modifiers.matches_exact(Modifiers::CTRL) => Some(TabStep::Next),
        Key::PageUp if modifiers.matches_exact(Modifiers::CTRL) => Some(TabStep::Previous),
        _ => None,
    }
}

/// Applies and consumes the tab shortcuts pressed this frame.
pub(super) fn apply_shortcuts(ui: &egui::Ui, tab: &mut DetailTab) {
    ui.input_mut(|input| {
        input.events.retain(|event| {
            if let egui::Event::Key {
                key,
                pressed: true,
                modifiers,
                ..
            } = *event
                && let Some(step) = tab_step(key, modifiers)
            {
                *tab = tab.stepped(step);
                return false;
            }
            true
        });
    });
}

/// Shows the tabs as text tabs in an accessibility tab list whose items
/// report selection instead of a toggle-button state, and returns the area
/// they occupy. Call it inside a pane header, whose height the tabs fill.
pub(super) fn show(ui: &mut egui::Ui, current: &mut DetailTab, lang: Language) -> egui::Rect {
    controls::tab_list(ui, lang.pick("Request details", "요청 상세"), |ui| {
        let mut area = egui::Rect::NOTHING;
        for tab in DetailTab::ALL {
            let selected = *current == tab;
            let response = controls::text_tab(ui, selected, tab.title(lang));
            if response.clicked() {
                *current = tab;
            }
            area = area.union(response.rect);
        }
        area
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::style::tokens::Layout;
    use crate::ui::workbench::chrome;

    #[test]
    fn steps_wrap_around_both_ends() {
        assert_eq!(DetailTab::Headers.stepped(TabStep::Next), DetailTab::Body);
        assert_eq!(DetailTab::Body.stepped(TabStep::Next), DetailTab::Timing);
        assert_eq!(DetailTab::Timing.stepped(TabStep::Next), DetailTab::Headers);
        assert_eq!(
            DetailTab::Headers.stepped(TabStep::Previous),
            DetailTab::Timing
        );
        assert_eq!(
            DetailTab::Body.stepped(TabStep::Previous),
            DetailTab::Headers
        );
    }

    #[test]
    fn maps_only_exact_tab_shortcuts() {
        assert_eq!(tab_step(Key::F6, Modifiers::NONE), Some(TabStep::Next));
        assert_eq!(
            tab_step(Key::PageDown, Modifiers::CTRL),
            Some(TabStep::Next)
        );
        assert_eq!(
            tab_step(Key::PageUp, Modifiers::CTRL),
            Some(TabStep::Previous)
        );
        // Linux and Windows deliver Ctrl as both `ctrl` and `command`.
        let linux_ctrl = Modifiers::CTRL | Modifiers::COMMAND;
        assert_eq!(tab_step(Key::PageDown, linux_ctrl), Some(TabStep::Next));
        assert_eq!(tab_step(Key::PageUp, linux_ctrl), Some(TabStep::Previous));
        // Plain paging belongs to the request list, and other modifier
        // combinations stay available to egui and the platform.
        assert_eq!(tab_step(Key::PageDown, Modifiers::NONE), None);
        assert_eq!(tab_step(Key::PageUp, Modifiers::NONE), None);
        assert_eq!(tab_step(Key::F6, Modifiers::SHIFT), None);
        assert_eq!(
            tab_step(Key::PageDown, Modifiers::CTRL | Modifiers::SHIFT),
            None
        );
        assert_eq!(tab_step(Key::Tab, Modifiers::CTRL), None);
    }

    fn key_press(key: Key, modifiers: Modifiers) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        }
    }

    #[test]
    fn exposes_tabs_and_consumes_only_tab_shortcuts() -> Result<(), Box<dyn std::error::Error>> {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let mut tab = DetailTab::Headers;
        let input = egui::RawInput {
            events: vec![
                key_press(Key::PageDown, Modifiers::CTRL | Modifiers::COMMAND),
                key_press(Key::PageDown, Modifiers::NONE),
            ],
            ..Default::default()
        };
        let mut remaining = Vec::new();
        let mut bar = egui::Rect::NOTHING;
        let mut output = ctx.run_ui(input, |ui| {
            apply_shortcuts(ui, &mut tab);
            remaining = ui.input(|input| input.events.clone());
            bar = chrome::pane_header(ui, "tabs", |ui| show(ui, &mut tab, Language::English));
        });
        // The tabs fill the pane header and leave the rest of the panel to
        // the details below it.
        assert!(
            (bar.height() - Layout::PANE_HEADER_HEIGHT).abs() < 1.0,
            "tab bar is {bar:?}"
        );
        assert_eq!(tab, DetailTab::Body);
        output.textures_delta.clear();
        assert!(matches!(
            remaining.as_slice(),
            [egui::Event::Key {
                key: Key::PageDown,
                modifiers: Modifiers::NONE,
                ..
            }]
        ));

        let tree = output
            .platform_output
            .accesskit_update
            .ok_or("missing accessibility tree")?;
        let node = |id| {
            tree.nodes
                .iter()
                .find(|(node_id, _)| *node_id == id)
                .map(|(_, node)| node)
        };
        let (_, list) = tree
            .nodes
            .iter()
            .find(|(_, node)| node.role() == egui::Role::TabList)
            .ok_or("missing tab list")?;
        assert_eq!(list.label(), Some("Request details"));
        let tabs: Vec<_> = list
            .children()
            .iter()
            .filter_map(|&id| node(id))
            .map(|tab| {
                (
                    tab.role(),
                    tab.label().unwrap_or_default().to_owned(),
                    tab.is_selected(),
                    tab.toggled(),
                )
            })
            .collect();
        assert_eq!(
            tabs,
            vec![
                (egui::Role::Tab, "Headers".to_owned(), Some(false), None),
                (egui::Role::Tab, "Body".to_owned(), Some(true), None),
                (egui::Role::Tab, "Timing".to_owned(), Some(false), None),
            ]
        );
        Ok(())
    }
}
