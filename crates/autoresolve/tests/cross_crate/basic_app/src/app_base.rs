//! Root `#[base]` for the basic application resolver.

use crate::app_context::AppContext;

/// Application root base spreading the cross-crate `Builtins` re-export.
#[derive(Debug)]
#[autoresolve::base(helper_module_exported_as = crate::app_base::app_base_helper)]
pub struct AppBase {
    /// Spread builtins from the runtime crate.
    #[spread]
    pub builtins: xc_runtime::core::Builtins,
    /// Application-level context value.
    pub app_context: AppContext,
}
