//! Execution of a validated replay request through a running proxy.

use std::sync::Arc;

use bytes::Bytes;
use http::{Request, StatusCode};
use http_body_util::{BodyExt, Full};

use super::capture_context::CaptureContext;
use super::forward::forward_http;
use super::inspect::forward_https;
use super::upstream::{connect_deadline, resolve_within};
use super::{EngineState, ProxyHandle, failed_response, infallible_to_hyper};
use crate::https::Destination;
use http_inspector_exchange::{CaptureSource, FailureStage};

/// Running replay; cancelling aborts the forwarding task.
pub struct ReplayTask(tokio::task::JoinHandle<()>);

impl ReplayTask {
    /// Cancels pending forwarding and response consumption.
    pub fn cancel(&self) {
        self.0.abort();
    }

    /// Whether forwarding and response consumption ended.
    #[must_use]
    pub fn is_finished(&self) -> bool {
        self.0.is_finished()
    }
}

/// Reason the running proxy cannot execute a replay request.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub(crate) enum ReplayExecutionError {
    #[error("Start the proxy before sending")]
    ProxyNotRunning,
    #[error("Enable HTTPS inspection before replaying HTTPS")]
    HttpsInspectionDisabled,
}

/// Executes a validated replay request on the running proxy, recording a new exchange.
pub(crate) fn execute_replay(
    proxy: &ProxyHandle,
    request: Request<Full<Bytes>>,
) -> Result<ReplayTask, ReplayExecutionError> {
    if !matches!(proxy.state(), EngineState::Running) {
        return Err(ReplayExecutionError::ProxyNotRunning);
    }
    if request.uri().scheme_str() == Some("https") && proxy.engine.inspector.is_none() {
        return Err(ReplayExecutionError::HttpsInspectionDisabled);
    }
    let engine = Arc::clone(&proxy.engine);
    Ok(ReplayTask(proxy.runtime.spawn(async move {
        let Ok(_permit) = Arc::clone(&engine.connections).acquire_owned().await else {
            return;
        };
        let target = request.uri().to_string();
        let source = if request.uri().scheme_str() == Some("https") {
            CaptureSource::InspectedHttps
        } else {
            CaptureSource::HttpProxy
        };
        let capture = CaptureContext::start(&request, source, &target, &engine);
        let request = request.map(|body| body.map_err(infallible_to_hyper).boxed());
        // Resolve before HTTPS as well, preserving the loop guard.
        let response = if source == CaptureSource::InspectedHttps {
            let reject = |message: &str| {
                failed_response(
                    &capture,
                    FailureStage::Request,
                    StatusCode::BAD_REQUEST,
                    message,
                );
            };
            let Some(authority) = request.uri().authority() else {
                return reject("the replay URL has no authority");
            };
            if let Err(failure) =
                resolve_within(connect_deadline(&engine), authority, 443, &engine).await
            {
                failure.respond(&capture);
                return;
            }
            let Ok(destination) = Destination::from_authority(authority) else {
                return reject("the replay URL authority is not a valid destination");
            };
            let Some(inspector) = engine.inspector.as_ref() else {
                return reject("HTTPS inspection was disabled before the replay started");
            };
            forward_https(request, &engine, &capture, inspector, &destination).await
        } else {
            forward_http(request, &engine, &capture).await
        };
        let mut body = response.into_body();
        while let Some(frame) = body.frame().await {
            if frame.is_err() {
                break;
            }
        }
    })))
}
