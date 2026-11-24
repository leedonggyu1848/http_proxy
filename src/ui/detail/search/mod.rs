//! UI search controls and match highlighting backed by
//! [`http_proxy::analysis::search`].

mod controller;
pub(super) mod highlight;

pub(crate) use controller::RequestSearch;
pub(super) use http_proxy::analysis::search::Query;
