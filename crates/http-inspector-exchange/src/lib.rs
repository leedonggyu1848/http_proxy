//! Shared HTTP capture models, HTTP semantics rules, capture preview rules,
//! and test fixtures for HTTP Inspector.
//!
//! This crate is runtime- and UI-agnostic: it depends only on `http` (plus
//! optional `thiserror` under the `test-support` feature) and carries no
//! `tokio`, `hyper`, or `eframe` dependency.

#![forbid(unsafe_code)]

pub mod capture;
mod exchange;
#[cfg(feature = "test-support")]
mod exchange_builder;
pub mod http_semantics;

#[cfg(feature = "test-support")]
pub use exchange::MemoryBodySource;
pub use exchange::{
    AuthScheme, BodyLifecycle, BodyPreview, BodyPreviewWriter, BodyRetention, BodySide,
    BodySnapshot, BodySource, ByteSize, CaptureSource, CapturedHeader, CapturedRequest,
    CapturedResponse, ClientConnection, ExactByteSize, Exchange, ExchangeWriter, FailureStage,
    HeaderPreview, ImageFormat, MediaType, Outcome, PhaseSpan, ProcessAncestor, ProcessIdentity,
    ProcessInfo, ProcessLookupWriter, ProcessUnavailable, ReadBodyOutcome, RecordId, SessionId,
    Timing, TunnelPreview, TunnelSnapshot, TunnelState, TunnelWriter,
};
#[cfg(feature = "test-support")]
pub use exchange_builder::{
    ConnectExchangeBuilder, ExchangeBuilder, FixtureError, ResponseFixture,
};

/// Application display name shared by exports, logs, and the desktop UI.
pub const APP_NAME: &str = "HTTP Inspector";
