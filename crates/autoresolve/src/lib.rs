mod base_type;
mod composite;
mod resolve_deps;
mod resolve_from;
mod resolver;
mod resolver_macro;
mod resolver_store;
mod scoped_resolver;
pub(crate) mod shared_type_map;

#[cfg(feature = "macros")]
pub use autoresolve_macros::base;
#[cfg(feature = "macros")]
pub use autoresolve_macros::composite;
#[cfg(feature = "macros")]
pub use autoresolve_macros::resolvable;
pub use base_type::BaseType;
pub use composite::CompositePart;
pub use resolve_deps::{ResolutionDeps, ResolutionDepsEnd, ResolutionDepsNode};
pub use resolve_from::ResolveFrom;
pub use resolver::Resolver;
pub use resolver_store::ResolverStore;
pub use scoped_resolver::{ScopedResolver, SharedResolver};
