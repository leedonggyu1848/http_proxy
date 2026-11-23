//! Which message of an exchange a body belongs to.

use std::fmt;

/// Which message of an exchange a body belongs to.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum BodySide {
    /// The request body sent by the client.
    Request,
    /// The response body sent by the upstream server.
    Response,
}

impl fmt::Display for BodySide {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Request => "Request",
            Self::Response => "Response",
        })
    }
}
