//! HTTP rules shared by capture, forwarding, logging, and export: which
//! fields are hop-by-hop and which carry credentials that are never retained.

mod hop_by_hop;
mod sensitive;

pub(crate) use hop_by_hop::remove_hop_by_hop_headers;
pub(crate) use sensitive::{
    REDACTED_VALUE, is_sensitive_header, is_sensitive_request_header, redacted_value,
};
