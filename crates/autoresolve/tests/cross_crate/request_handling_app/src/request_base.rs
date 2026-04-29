//! Request-tier base scoped on `AppBase`.

use xc_http::request::Request;

pub use crate::app_base::AppBase;

/// Request-tier base. Scoped beneath [`AppBase`].
#[derive(Debug)]
#[autoresolve::base(
    scoped(AppBase),
    helper_module_exported_as = crate::request_base::request_base_helper
)]
pub struct RequestBase {
    /// Per-request HTTP request value.
    pub request: Request,
}
