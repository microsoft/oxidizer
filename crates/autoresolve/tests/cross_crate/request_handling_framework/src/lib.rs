//! Framework fixture: declares `FrameworkBase` that `#[spread]`s the
//! re-exported `xc_runtime::core::Builtins`. Application crates can scope on
//! top of this base.

pub mod framework_base;
pub mod framework_context;
