// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use alloc::borrow::Cow;
use alloc::sync::Arc;
use core::fmt;

use smallvec::SmallVec;

use crate::raw_match::INLINE_CAPTURES;

/// Matches requests to the routes of a resolver.
///
/// Code that reports the route of a request, such as telemetry, uses this
/// trait. It reads the route template, such as `/books/{book}`, and the raw
/// values of the route parameters. Such code can keep a resolver as
/// `Arc<dyn RouteTable + Send + Sync>`.
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
/// let hit = resolver
///     .match_route("GET", "/books/rust")
///     .expect("a route matches");
/// assert_eq!(hit.template().as_str(), "/books/{book}");
/// assert_eq!(hit.parameter("book"), Some("rust"));
/// ```
pub trait RouteTable {
    /// Matches `method` and `path` to a route.
    ///
    /// The match picks the same route as the `resolve` method of the resolver.
    /// It does not convert the parameter values to typed values, so a route
    /// matches even if such a conversion fails. Deep paths or many parameter
    /// values can allocate.
    ///
    /// Returns [`None`] if no route matches or if `path` contains `?` or `#`.
    fn match_route<'p>(&self, method: &str, path: &'p str) -> Option<RouteHit<'_, 'p>>;
}

/// The template of a route, such as `/books/{book}`.
///
/// A clone does not copy the text, so a caller can keep the template of a match.
#[derive(Clone, Debug)]
pub enum RouteTemplate {
    /// The template of a route that [`#[resolver]`](macro@crate::resolver) generates.
    Static(&'static str),
    /// The template of a route that is added at run time. All matches of the route share it.
    Shared(Arc<str>),
}

impl RouteTemplate {
    /// Returns the text of the template.
    #[must_use]
    pub fn as_str(&self) -> &str {
        match self {
            Self::Static(text) => text,
            Self::Shared(text) => text,
        }
    }
}

impl AsRef<str> for RouteTemplate {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Display for RouteTemplate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A route that matches a request, with the values of its parameters.
#[derive(Clone, Debug)]
pub struct RouteHit<'r, 'p> {
    pub(crate) template: &'r RouteTemplate,
    pub(crate) names: &'r [Cow<'static, str>],
    pub(crate) captures: SmallVec<[&'p str; INLINE_CAPTURES]>,
}

impl<'r, 'p> RouteHit<'r, 'p> {
    /// Returns the template of the route, such as `/books/{book}`.
    #[must_use]
    pub const fn template(&self) -> &'r RouteTemplate {
        self.template
    }

    /// Returns the value of the route parameter `name`, or [`None`] if the
    /// route has no such parameter.
    ///
    /// The name of a dotted parameter, such as `{book.id}`, is `book.id`. The
    /// value is a part of the request path, still percent-encoded.
    #[must_use]
    pub fn parameter(&self, name: &str) -> Option<&'p str> {
        let index = self.names.iter().position(|candidate| candidate == name)?;
        self.captures.get(index).copied()
    }
}

/// Builds the [`RouteHit`] that a generated resolver returns.
#[inline]
#[must_use]
pub fn route_hit<'r, 'p>(template: &'r RouteTemplate, names: &'r [Cow<'static, str>], captures: &[&'p str]) -> RouteHit<'r, 'p> {
    RouteHit {
        template,
        names,
        captures: SmallVec::from_slice(captures),
    }
}
