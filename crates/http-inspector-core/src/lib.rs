//! Runtime proxy core for HTTP Inspector: local HTTP/HTTPS interception,
//! CA and OS trust store management, session recording and body storage,
//! request replay, browser and terminal launching, inspection shell
//! environments, and settings.
//!
//! This crate owns async networking (`tokio`, `hyper`, `rustls`) and OS
//! integration while remaining free of any UI (`eframe`/`egui`) dependency.

#![forbid(unsafe_code)]

pub mod hls;
pub mod https;
pub mod interception;
pub mod launcher;
pub mod process;
pub mod recording;
pub mod replay;
pub mod settings;
pub mod shell;

/// Default loopback proxy listener port.
pub const DEFAULT_PORT: u16 = 8080;
