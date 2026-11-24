#![forbid(unsafe_code)]

pub mod analysis;
pub mod content;
pub mod exchange;
#[expect(
    clippy::redundant_pub_crate,
    reason = "crate-wide items of a private module; `pub` would trip `unreachable_pub`"
)]
mod http_semantics;
pub mod https;
pub mod interception;
pub mod interop;
pub mod launcher;
pub mod recording;
pub mod replay;
pub mod settings;

/// Application display name.
pub const APP_NAME: &str = "HTTP Inspector";

/// Default loopback proxy listener port.
pub const DEFAULT_PORT: u16 = 8080;
