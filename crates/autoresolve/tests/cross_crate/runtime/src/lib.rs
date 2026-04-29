//! Runtime fixture: re-exports `Builtins` from `xc_scheduler` via
//! `#[reexport_base]`, exercising cross-crate base re-exports.

#![allow(missing_docs, missing_debug_implementations)]

pub mod core;
mod internal;
