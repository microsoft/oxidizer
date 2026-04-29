use crate::framework_context::FrameworkContext;

#[autoresolve::base(helper_module_exported_as = crate::framework_base::framework_base_helper)]
pub struct FrameworkBase {
    #[spread]
    pub builtins: xc_runtime::core::Builtins,
    pub framework_context: FrameworkContext,
}
