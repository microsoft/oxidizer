//! Root `#[base]` that spreads the cross-crate `Builtins` re-export.

use crate::framework_context::FrameworkContext;

/// Root resolver base, spreading [`xc_runtime::core::Builtins`] and adding
/// a [`FrameworkContext`].
#[derive(Debug)]
#[autoresolve::base(helper_module_exported_as = crate::framework_base::framework_base_helper)]
pub struct FrameworkBase {
    /// Spread builtins from the runtime crate.
    #[spread]
    pub builtins: xc_runtime::core::Builtins,
    /// Framework-wide context value.
    pub framework_context: FrameworkContext,
}
