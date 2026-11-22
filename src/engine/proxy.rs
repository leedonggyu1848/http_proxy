use crate::engine::domain::FailureStage;
use bytes::Bytes;
use capture_context::CaptureContext;
use http::{Response, StatusCode};
use http_body_util::combinators::BoxBody;
use http_body_util::{BodyExt, Empty, Full};
use std::convert::Infallible;

mod capture_context;
mod config;
mod forward;
mod handle;
pub(in crate::engine) mod headers;
mod inspect;
mod replay;
mod status;
mod target;
mod tunnel;
mod upstream;

pub use config::ProxyConfig;
pub use handle::{ProxyHandle, StartError, StopError};
pub use replay::{REPLAY_BODY_FILE_LIMIT_BYTES, ReplayDraft, ReplayField, ReplayTask};
pub use status::EngineState;

type ProxyBody = BoxBody<Bytes, hyper::Error>;

fn failed_response(
    capture: &CaptureContext,
    stage: FailureStage,
    status: StatusCode,
    message: &str,
) -> Response<ProxyBody> {
    let message = capture.fail(stage, message);
    text_response(status, &message)
}

fn empty_response(status: StatusCode) -> Response<ProxyBody> {
    Response::builder()
        .status(status)
        .body(Empty::<Bytes>::new().map_err(infallible_to_hyper).boxed())
        .unwrap_or_else(|_| {
            Response::new(Empty::<Bytes>::new().map_err(infallible_to_hyper).boxed())
        })
}

fn text_response(status: StatusCode, message: &str) -> Response<ProxyBody> {
    Response::builder()
        .status(status)
        .body(
            Full::new(Bytes::copy_from_slice(message.as_bytes()))
                .map_err(infallible_to_hyper)
                .boxed(),
        )
        .unwrap_or_else(|_| {
            Response::new(Empty::<Bytes>::new().map_err(infallible_to_hyper).boxed())
        })
}

const fn infallible_to_hyper(error: Infallible) -> hyper::Error {
    match error {}
}
