pub(in crate::ui) mod app;
pub(in crate::ui) mod detail;
pub(in crate::ui) mod dialogs;
pub(in crate::ui) mod map;
pub(in crate::ui) mod requests;
pub(in crate::ui) mod style;

pub use app::main;
#[cfg(test)]
pub(in crate::ui) use app::request_shortcuts_allowed;
pub(in crate::ui) use app::{InspectorApp, MAX_RECORDS};
