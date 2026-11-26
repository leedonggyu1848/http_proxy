//! The docked workbench shell: chrome bands (toolbar, pane headers, status
//! bar), the controls drawn on them, and the strings they show.
//!
//! Screens decide what goes into a band; this module decides how a band,
//! tab, segmented control, ghost or primary button, and input look, so that
//! every pane follows `docs/ui-design-system.md` (D22, D25) the same way.

pub(crate) mod chrome;
pub(crate) mod controls;
pub(crate) mod field;
pub(crate) mod text;
