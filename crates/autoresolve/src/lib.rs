//! Compile-time dependency injection framework for Rust.
//!
//! `autoresolve` provides a [`Resolver`] that lazily constructs types from their
//! declared dependencies. Annotate an `impl` block with [`#[resolvable]`](resolvable)
//! to declare how a type is constructed, and use [`#[base]`](base) to define the
//! root types that seed the resolver.

mod base_type;
mod resolve_deps;
mod resolve_from;
mod resolver;
mod resolver_macro;
mod resolver_store;
pub(crate) mod shared_type_map;

#[cfg(feature = "macros")]
pub use autoresolve_macros::base;
#[cfg(feature = "macros")]
pub use autoresolve_macros::reexport_base;
#[cfg(feature = "macros")]
pub use autoresolve_macros::resolvable;
pub use base_type::BaseType;
pub use resolve_deps::{ResolutionDeps, ResolutionDepsEnd, ResolutionDepsNode};
pub use resolve_from::ResolveFrom;
pub use resolver::Resolver;
pub use resolver_store::ResolverStore;
