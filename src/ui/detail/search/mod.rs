//! UI search controls and match highlighting backed by
//! [`http_inspector_analysis::search`].

mod controller;
mod controls;
pub(super) mod highlight;

pub(crate) use controller::{RequestSearch, SearchSettings};
pub(super) use http_inspector_analysis::search::Query;
