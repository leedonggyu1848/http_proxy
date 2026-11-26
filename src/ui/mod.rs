#[expect(
    clippy::redundant_pub_crate,
    reason = "cross-screen items in a private module; `pub` triggers `unreachable_pub`"
)]
mod app;
#[expect(
    clippy::redundant_pub_crate,
    reason = "cross-screen items in a private module; `pub` triggers `unreachable_pub`"
)]
mod detail;
#[expect(
    clippy::redundant_pub_crate,
    reason = "cross-screen items in a private module; `pub` triggers `unreachable_pub`"
)]
mod dialogs;
#[expect(
    clippy::redundant_pub_crate,
    reason = "cross-screen items in a private module; `pub` triggers `unreachable_pub`"
)]
mod i18n;
#[expect(
    clippy::redundant_pub_crate,
    reason = "cross-screen items in a private module; `pub` triggers `unreachable_pub`"
)]
mod map;
#[expect(
    clippy::redundant_pub_crate,
    reason = "cross-screen items in a private module; `pub` triggers `unreachable_pub`"
)]
mod requests;
#[expect(
    clippy::redundant_pub_crate,
    reason = "cross-screen items in a private module; `pub` triggers `unreachable_pub`"
)]
mod style;

pub use app::main;
#[cfg(test)]
use app::request_shortcuts_allowed;
use app::{AppAction, InspectorApp, MAX_RECORDS};
