// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use crate::ResolveError;

/// Resolves an HTTP method and path to a typed route.
///
/// [`resolver`](macro@crate::resolver) generates a concrete implementation for
/// each route enum. Static-only route enums provide an infallible `resolver`
/// constructor; route enums with dynamic variants provide a builder.
///
/// ```
/// use routerama::{ResolveError, Resolver};
///
/// #[routerama::resolver]
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
/// ```
pub trait Resolver {
    /// The route enum produced for a request path borrowed for `'p`.
    type Route<'p>;

    /// Resolves an HTTP `method` + `path` into the route enum.
    ///
    /// Static routes are scanned first and dynamic routes are consulted only
    /// after a static miss.
    ///
    /// Resolution is linear in the request-path length. Request input cannot
    /// increase traversal recursion beyond the statically or dynamically
    /// configured route depth.
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
