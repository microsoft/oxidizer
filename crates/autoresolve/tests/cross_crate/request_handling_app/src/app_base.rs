use crate::app_context::AppContext;
pub use xc_request_handling_framework::framework_base::FrameworkBase;

#[autoresolve::base(
    scoped(FrameworkBase),
    helper_module_exported_as = crate::app_base::app_base_helper
)]
pub struct AppBase {
    pub req_app_context: AppContext,
}
