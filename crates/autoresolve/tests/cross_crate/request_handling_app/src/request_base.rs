use xc_http::request::Request;

pub use crate::app_base::AppBase;

#[autoresolve::base(
    scoped(AppBase),
    helper_module_exported_as = crate::request_base::request_base_helper
)]
pub struct RequestBase {
    pub request: Request,
}
