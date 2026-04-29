//! Framework fixture: declares `FrameworkBase` that `#[spread]`s the
//! re-exported `xc_runtime::core::Builtins`. Application crates can scope on
//! top of this base.

#![allow(missing_docs, missing_debug_implementations)]

pub mod framework_context {
    #[derive(Clone)]
    pub struct FrameworkContext;
}

pub mod framework_base {
    use crate::framework_context::FrameworkContext;

    #[autoresolve::base(helper_module_exported_as = crate::framework_base::framework_base_helper)]
    pub struct FrameworkBase {
        #[spread]
        pub builtins: xc_runtime::core::Builtins,
        pub framework_context: FrameworkContext,
    }
}
