//! HTTP rules shared by capture, forwarding, and export.

mod hop_by_hop;

pub(crate) use hop_by_hop::remove_hop_by_hop_headers;
