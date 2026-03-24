use autoresolve_macros::base;

pub mod request;

use request::Request;

#[base(scoped(crate::AppBase), helper_module_exported_as = crate::http::request_base_helper)]
pub struct RequestBase {
    pub request: Request,
}
