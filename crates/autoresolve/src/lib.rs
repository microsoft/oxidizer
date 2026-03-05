mod composite;
mod resolve_deps;
mod resolve_from;
mod resolver;
mod resolver_macro;

#[cfg(feature = "macros")]
pub use autoresolve_macros::composite;
#[cfg(feature = "macros")]
pub use autoresolve_macros::resolvable;
pub use composite::CompositePart;
pub use resolve_deps::{ResolutionDeps, ResolutionDepsEnd, ResolutionDepsNode};
pub use resolve_from::ResolveFrom;
pub use resolver::Resolver;
