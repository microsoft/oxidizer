use autoresolve_macros::base;

pub mod request;

#[base(scoped(super::app_base::AppBase))]
pub mod request_base {
    pub struct RequestBase {
        pub request: super::request::Request,
    }
}
