//! HTTP fixture: defines `HttpClient` whose `#[resolvable]` constructor pulls
//! dependencies from sibling crates `xc_io_driver` and `xc_scheduler`.

#![allow(missing_docs, missing_debug_implementations)]

pub mod client;
pub mod request;
