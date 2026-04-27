//! Compile-time dependency injection framework for Rust.
//!
//! `autoresolve` provides a [`Resolver`] that lazily constructs types from their
//! declared dependencies. Annotate an `impl` block with [`#[resolvable]`](resolvable)
//! to declare how a type is constructed, and use [`#[base]`](base) to define the
//! root types that seed the resolver.

mod base_type;
mod dependency_of;
mod path_cache;
mod path_stack;
mod provide;
mod provide_path;
mod resolve_deps;
mod resolve_from;
mod resolve_output;
mod resolver;
mod resolver_macro;

#[cfg(feature = "macros")]
pub use autoresolve_macros::base;
#[cfg(feature = "macros")]
pub use autoresolve_macros::reexport_base;
#[cfg(feature = "macros")]
pub use autoresolve_macros::resolvable;
pub use base_type::BaseType;
pub use dependency_of::DependencyOf;
pub use path_stack::PathStack;
pub use provide::{BranchBuilder, Branched, ProvideBuilder};
pub use provide_path::{Scoped, Unscoped};
pub use resolve_deps::{ResolutionDeps, ResolutionDepsEnd, ResolutionDepsNode};
pub use resolve_from::ResolveFrom;
pub use resolve_output::ResolveOutput;
pub use resolver::Resolver;
