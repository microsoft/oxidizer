use crate::app_context::AppContext;

#[autoresolve::base(helper_module_exported_as = crate::app_base::app_base_helper)]
pub struct AppBase {
    #[spread]
    pub builtins: xc_runtime::core::Builtins,
    pub app_context: AppContext,
}
