// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

/// The result of resolving a request: a matched route's name and captured path
/// variables.
///
/// Implemented by the generated route `enum` and (under the `dynamic` feature)
/// by `DynMatch`, so code can handle either uniformly. It requires no
/// `Copy`/`&'static`, so a runtime match with heap-allocated names and a
/// runtime-sized capture list fits too.
pub trait RouteMatch<'p> {
    /// The matched route's name (the generated enum variant name, or the name a
    /// `Route` was registered under).
    fn name(&self) -> &str;

    /// The captured value of the path variable `name`, or [`None`] if the matched
    /// route has no such variable.
    ///
    /// `name` is the template variable's name exactly as written — including
    /// dotted (`shelf.id`), keyword (`type`), or otherwise non-identifier
    /// (`a-b`) names. (The static backend sanitizes those into valid Rust
    /// *field* identifiers for pattern matching, but `capture` keys on the
    /// original name so both backends accept the same string.)
    fn capture(&self, name: &str) -> Option<&'p str>;
}

/// A resolver: maps an HTTP method + path to a [`RouteMatch`].
///
/// Every way of resolving implements this: the zero-sized resolver
/// `#[resolver]` generates, the runtime [`DynResolver`](crate::DynResolver),
/// and their [`EitherResolver`](crate::EitherResolver) composition. It is the
/// single entry point — a route `enum` is only the *result* ([`RouteMatch`]),
/// not a resolver.
///
/// `resolve` borrows `&'p self` so a match may borrow both the resolver (route
/// names, capture keys) and the request path (captured values) under one
/// lifetime. A generated static resolver is a zero-sized type; a `DynResolver`
/// carries a runtime trie.
pub trait Resolver {
    /// The match this resolver produces.
    type Match<'p>: RouteMatch<'p>
    where
        Self: 'p;

    /// Resolves `method` + `path`, returning the matched route or [`None`].
    ///
    /// `method` is matched by exact, case-sensitive token (e.g. `"GET"`), so pass
    /// the request method verbatim. [`HttpMethod`](crate::HttpMethod) may be
    /// passed directly, as it implements [`AsRef<str>`].
    fn resolve<'p, P>(&'p self, method: impl AsRef<str>, path: &'p P) -> Option<Self::Match<'p>>
    where
        P: AsRef<str> + ?Sized;
}
