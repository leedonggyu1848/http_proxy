//! Local HTTP/HTTPS proxy listener, forwarding, CONNECT tunneling, and capture.

use bytes::Bytes;
use capture_context::CaptureContext;
use http::{Response, StatusCode};
use http_body_util::combinators::BoxBody;
use http_body_util::{BodyExt, Empty, Full};
use http_inspector_exchange::FailureStage;
use std::convert::Infallible;

mod capture;
mod capture_context;
mod config;
#[expect(
    clippy::redundant_pub_crate,
    reason = "crate-wide items of a private module; `pub` would trip `unreachable_pub`"
)]
mod execute;
mod forward;
mod handle;
mod inspect;
mod pool;
mod process_lookup;
mod status;
mod target;
mod tunnel;
mod upstream;

pub use config::ProxyConfig;
pub use execute::ReplayTask;
pub(crate) use execute::{ReplayExecutionError, execute_replay};
pub use handle::{ProxyHandle, StartError, StopError};
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
