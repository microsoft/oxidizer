// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use core::fmt::{Debug, Display};

/// A resolved HTTP route.
///
/// This trait is implemented by every router this crate generates.
///
/// # Examples
///
/// Code generic over any router — returns the first path that matches:
///
/// ```
/// use routerama::Route;
///
/// fn first_match<'p, R: Route<'p>>(method: &str, paths: &[&'p str]) -> Option<R> {
///     paths
///         .iter()
///         .copied()
///         .find_map(|path| R::resolve(method, path))
/// }
/// ```
pub trait Route<'p>: Copy + Debug + Display {
    /// Resolves an HTTP `method` + `path` to the route it matches, capturing any
    /// path variables in the matched variant's fields.
    fn resolve<P>(method: impl AsRef<str>, path: &'p P) -> Option<Self>
    where
        P: AsRef<str> + ?Sized;

    /// This route's name as the string it was declared with, independent of any
    /// captured variable values.
    fn name(&self) -> &'static str;
}

/// The result of resolving a request: a matched route's name and captured path
/// variables.
///
/// Implemented by both the static generated route enum and (under the `dynamic`
/// feature) `DynMatch`, so code can dispatch over either uniformly. Unlike
/// [`Route`], it does not require `Copy`/`&'static`, so a runtime match with
/// heap-allocated names and a runtime-sized capture list also fits.
pub trait RouteMatch<'p> {
    /// The matched route's name (the generated enum variant name, or the name a
    /// `RouteRule` was registered under).
    fn name(&self) -> &str;

    /// The captured value of the path variable `name`, or [`None`] if the matched
    /// route has no such variable. `name` is the field-name form of the template
    /// variable (dotted names joined with `_`, e.g. `shelf.id` → `shelf_id`).
    fn capture(&self, name: &str) -> Option<&'p str>;
}

/// A resolver: maps an HTTP method + path to a [`RouteMatch`].
///
/// This is the *router* half of the abstraction that [`Route`] fuses into one
/// type. Splitting it out lets a value-carrying runtime router (a `DynRouter`)
/// and a zero-sized generated static router implement the same trait, and lets
/// them compose (an `EitherRouter`).
///
/// `resolve` borrows `&'p self` so a match may borrow both the router (route
/// names, capture keys) and the request path (captured values) under one
/// lifetime. A static generated router is a zero-sized type; a `DynRouter`
/// carries a runtime trie.
pub trait Router {
    /// The match this router produces.
    type Match<'p>: RouteMatch<'p>
    where
        Self: 'p;

    /// Resolves `method` + `path`, returning the matched route or [`None`].
    fn resolve<'p>(&'p self, method: &str, path: &'p str) -> Option<Self::Match<'p>>;
}
