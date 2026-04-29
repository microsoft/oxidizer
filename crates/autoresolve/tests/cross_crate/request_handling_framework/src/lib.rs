//! Framework fixture: declares `FrameworkBase` that `#[spread]`s the
//! re-exported `xc_runtime::core::Builtins`. Application crates can scope on
//! top of this base.

#![allow(missing_docs, missing_debug_implementations)]

pub mod framework_base;
pub mod framework_context;
