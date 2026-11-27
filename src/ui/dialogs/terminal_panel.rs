//! The toolbar's Open Terminal button: a new terminal window running an
//! inspection shell for the app's proxy.
//!
//! The window runs this executable's `shell` command, which builds the
//! proxy and CA variables and removes its CA bundle when the shell exits.
//! The terminal is independent of the app and stays open after it quits.

use std::net::SocketAddr;
use std::path::Path;
use std::time::Duration;

use eframe::egui;
use http_inspector_core::launcher::{Terminal, TerminalLaunch, TerminalLaunchError};

use crate::ui::i18n::{Language, domain};
use crate::ui::style::tokens::Layout;
use crate::ui::workbench::controls;

#[derive(Default)]
pub(crate) struct TerminalPanel {
    launch: Option<TerminalLaunch>,
    error: Option<TerminalLaunchError>,
}

impl TerminalPanel {
    /// The Open Terminal button. Disabled while the proxy is stopped or a
    /// terminal is starting. Returns whether it was clicked.
    pub(crate) fn toolbar(&self, ui: &mut egui::Ui, running: bool, lang: Language) -> bool {
        let text = lang.terminal();
        if self.launch.is_some() {
            ui.ctx().request_repaint_after(Duration::from_millis(100));
        }
        controls::ghost_scope(ui, |ui| {
            ui.add_enabled(
                running && self.launch.is_none(),
                controls::ghost(text.open_terminal, Layout::TOOLBAR_BUTTON_HEIGHT),
            )
            .on_hover_text(text.tooltips.opens_shell)
            .on_disabled_hover_text(if running {
                text.tooltips.opens_shell
            } else {
                text.tooltips.start_proxy_first
            })
            .clicked()
        })
    }

    /// Opens a terminal for `proxy`; `ca` is the CA directory while HTTPS
    /// is decrypted.
    pub(crate) fn open(&mut self, proxy: SocketAddr, ca: Option<&Path>) {
        self.error = None;
        let Some(terminal) = Terminal::find() else {
            self.error = Some(TerminalLaunchError::NotFound);
            return;
        };
        let launched = std::env::current_exe().and_then(|program| {
            terminal.open(&program, &http_inspector_cli::terminal_arguments(proxy, ca))
        });
        match launched {
            Ok(launch) => self.launch = Some(launch),
            Err(error) => self.error = Some(TerminalLaunchError::Io(error)),
        }
    }

    /// Collects the launch result; reports whether anything changed.
    pub(crate) fn receive(&mut self) -> bool {
        let Some(result) = self.launch.as_mut().and_then(TerminalLaunch::try_receive) else {
            return false;
        };
        self.launch = None;
        self.error = result.err();
        true
    }

    /// The last launch error, shown below the toolbar.
    pub(crate) fn error(&self, lang: Language) -> Option<String> {
        self.error
            .as_ref()
            .map(|error| domain::terminal_launch_error(lang, error))
    }

    /// Forgets the last error, as when the proxy stops.
    pub(crate) fn clear_error(&mut self) {
        self.error = None;
    }
}
