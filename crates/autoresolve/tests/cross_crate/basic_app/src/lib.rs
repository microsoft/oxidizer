//! Basic application fixture: a flat resolver rooted at `AppBase` that
//! `#[spread]`s the cross-crate `Builtins` re-export and resolves an
//! `AppService` whose deps come from sibling crates.

pub mod app_base;
pub mod app_context;
pub mod app_service;

#[cfg(test)]
mod tests;
