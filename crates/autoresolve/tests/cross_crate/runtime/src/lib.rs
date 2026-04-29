//! Runtime fixture: re-exports `Builtins` from `xc_scheduler` via
//! `#[reexport_base]`, exercising cross-crate base re-exports.

pub mod core;
mod internal;
