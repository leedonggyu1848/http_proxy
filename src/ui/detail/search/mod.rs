//! UI search controls and match highlighting backed by
//! [`http_proxy::analysis::search`].

mod controller;
pub(in crate::ui) mod highlight;

pub(in crate::ui) use controller::RequestSearch;
pub(in crate::ui) use http_proxy::analysis::search::Query;
