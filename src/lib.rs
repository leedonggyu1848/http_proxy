#![forbid(unsafe_code)]

mod domain;
mod proxy;

pub use domain::{CaptureOutcome, CaptureRecord, FailureStage};
pub use proxy::{ProxyConfig, ProxyHandle, StartError, StopError};

/// Application display name.
pub const APP_NAME: &str = "HTTP Inspector";
