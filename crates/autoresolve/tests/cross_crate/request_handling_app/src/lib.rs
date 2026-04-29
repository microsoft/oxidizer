//! Request-handling application fixture: a 4-tier scoped resolver chain
//! spanning four different crates.
//!
//! Tier walk-down (root → leaf):
//! - `FrameworkBase` (in `xc_request_handling_framework`)
//! - `AppBase` scoped on `FrameworkBase` (this crate)
//! - `RequestBase` scoped on `AppBase` (this crate)
//! - `TaskBase` scoped on `RequestBase` (this crate)

#![allow(missing_docs, missing_debug_implementations)]

pub mod app_base;
pub mod app_context;
pub mod app_service;
pub mod request_base;
pub mod request_service;
pub mod task;

#[cfg(test)]
mod tests;
