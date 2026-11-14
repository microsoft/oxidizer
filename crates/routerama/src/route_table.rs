// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use alloc::borrow::Cow;

use smallvec::SmallVec;

use crate::raw_match::INLINE_CAPTURES;

/// Lists the routes of a resolver and matches requests to them.
///
/// Code that reports the route of a request, such as telemetry, uses this
/// trait. It reads the route template, such as `/books/{book}`, and the raw
/// values of the route parameters. Such code can keep a resolver as
/// `Arc<dyn RouteTable + Send + Sync>`.
///
/// Each route has a number from `0` to `route_count() - 1`. The number links a
/// match to its route: [`match_route`](Self::match_route) returns the number of
/// the route that matches, and [`template`](Self::template) and
/// [`parameters`](Self::parameters) take it. Which route has which number is
/// not specified.
///
/// Every resolver that [`#[resolver]`](macro@crate::resolver) generates
/// implements this trait.
///
/// # Examples
///
/// ```
/// use routerama::{RouteTable, resolver};
///
/// #[resolver]
/// enum BookRoute<'p> {
///     #[route(GET, "/books")]
///     ListBooks,
///     #[route(GET, "/books/{book}")]
///     GetBook { book: &'p str },
/// }
///
/// let resolver = BookRoute::resolver();
///
/// for route in 0..resolver.route_count() {
///     let template = resolver
///         .template(route)
///         .expect("every number below the count has a route");
///     let parameters = resolver
///         .parameters(route)
///         .expect("every number below the count has a route");
///     println!("{template} {parameters:?}");
/// }
///
/// let hit = resolver
///     .match_route("GET", "/books/rust")
///     .expect("a route matches");
/// assert_eq!(resolver.template(hit.route()), Some("/books/{book}"));
/// assert_eq!(hit.captures(), ["rust"]);
/// ```
pub trait RouteTable {
    /// Returns how many routes the resolver has.
    fn route_count(&self) -> usize;

    /// Returns the template of the route with the number `route`, such as
    /// `/books/{book}`, or [`None`] if no route has this number.
    fn template(&self, route: usize) -> Option<&str>;

    /// Returns the parameter names of the route with the number `route`, in
    /// the order of the template, or [`None`] if no route has this number.
    ///
    /// The name of a dotted parameter, such as `{book.id}`, is `book.id`.
    fn parameters(&self, route: usize) -> Option<&[Cow<'static, str>]>;

    /// Matches `method` and `path` to a route.
    ///
    /// The match picks the same route as the `resolve` method of the resolver.
    /// It does not convert the parameter values to typed values, so a route
    /// matches even if such a conversion fails. Deep paths or many parameter
    /// values may allocate.
    ///
    /// Returns [`None`] if no route matches or if `path` contains `?` or `#`.
    fn match_route<'p>(&self, method: &str, path: &'p str) -> Option<RouteHit<'p>>;
}

/// A route that matches a request, with the values of its parameters.
#[derive(Clone, Debug)]
pub struct RouteHit<'p> {
    pub(crate) route: usize,
    pub(crate) captures: SmallVec<[&'p str; INLINE_CAPTURES]>,
}

impl<'p> RouteHit<'p> {
    /// Returns the number of the route, which [`RouteTable::template`] and
    /// [`RouteTable::parameters`] take.
    #[must_use]
    pub const fn route(&self) -> usize {
        self.route
    }

    /// Returns the values of the route parameters, in the order of the
    /// parameter names of the route.
    ///
    /// Each value is a part of the request path, still percent-encoded.
    #[must_use]
    pub fn captures(&self) -> &[&'p str] {
        &self.captures
    }
}

/// Builds the [`RouteHit`] that a generated resolver returns for the route
/// with the number `route`.
#[inline]
#[must_use]
pub fn route_hit<'p>(route: usize, captures: &[&'p str]) -> RouteHit<'p> {
    RouteHit {
        route,
        captures: SmallVec::from_slice(captures),
    }
}
