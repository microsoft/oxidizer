// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use crate::ResolveError;

/// Resolves an HTTP method and path to a typed route.
///
/// The `resolver` macro generates a concrete implementation for
/// each route enum. Static-only route enums provide an infallible `resolver`
/// constructor; route enums with dynamic variants provide a builder.
///
/// ```
/// # #[cfg(feature = "resolve")]
/// # fn main() {
/// use routerama::resolve::{ResolveError, Resolver};
///
/// #[routerama::resolve::resolver]
/// enum AppRoute {
///     #[route(GET, "/")]
///     Home,
/// }
///
/// fn resolve_get<'p, R: Resolver>(
///     resolver: &R,
///     path: &'p str,
/// ) -> Result<R::Route<'p>, ResolveError<'p>> {
///     resolver.resolve("GET", path)
/// }
///
/// let resolver = AppRoute::resolver();
/// assert!(matches!(resolve_get(&resolver, "/"), Ok(AppRoute::Home)));
/// # }
/// # #[cfg(not(feature = "resolve"))]
/// # fn main() {}
/// ```
pub trait Resolver {
    /// The route enum produced for a request path borrowed for `'p`.
    type Route<'p>;

    /// Resolves an HTTP `method` + `path` into the route enum.
    ///
    /// Static routes are scanned first and dynamic routes are consulted only
    /// after a static miss.
    ///
    /// Resolution is linear in the request-path length for route tables built
    /// only from literal and single-segment-wildcard edges. Prefix/suffix
    /// (affix) edges are matched by scanning a node's affix edges, so a request
    /// reaching such a node also costs that node's affix fanout times the
    /// segment length. Request input cannot increase traversal recursion beyond
    /// the statically or dynamically configured route depth.
    ///
    /// # Errors
    ///
    /// Returns [`ResolveError::InvalidPath`] when `path` contains a query or
    /// fragment delimiter, [`ResolveError::NotFound`] when no route matches, or
    /// a capture variant when a matched route's capture cannot be decoded or
    /// converted to its declared field type.
    fn resolve<'p, P>(&self, method: impl AsRef<str>, path: &'p P) -> Result<Self::Route<'p>, ResolveError<'p>>
    where
        P: AsRef<str> + ?Sized;
}
