//! Implementation of proc macros for the `autoresolve` dependency injection framework.
//!
//! This crate provides the token-stream transformations used by `autoresolve_macros`.
//! It is not intended to be used directly.

mod base;
mod resolvable;

/// Generates a `BaseType` impl and helper macro for a struct annotated with `#[base]`.
pub use base::base;
/// Re-exports a `#[base]` struct from a different module path.
pub use base::reexport_base;
/// Generates a `ResolveFrom` impl for a type's `fn new(...)` constructor.
pub use resolvable::resolvable;
