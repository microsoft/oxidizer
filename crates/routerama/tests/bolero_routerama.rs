// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Bolero property/fuzz harnesses for the router.
//!
//! Two targets share one route table:
//!
//! * `resolve_never_diverges_on_arbitrary_input` — feeds unstructured,
//!   possibly-adversarial paths (arbitrary UTF-8, embedded `:`/`/`, non-ASCII
//!   straddling SIMD lane boundaries) through both backends. It must never
//!   panic — exercising the unchecked `scan_segments`/`seg_bytes`/`substr`
//!   helpers under sanitizers — and the two backends must agree.
//! * `dynamic_matches_static_on_structured_paths` — builds paths from a small
//!   alphabet of colliding segments (literals that also appear as wildcards,
//!   custom verbs, empty/trailing segments) so most inputs *hit* a route, then
//!   asserts the static (`#[resolver]`) oracle and the runtime
//!   `DynResolver` agree on the resolved name *and* every captured variable.
//!
//! Bolero corpus replay needs filesystem isolation that Miri does not provide,
//! so the whole harness is gated out of Miri; the unsafe scan helpers are
//! independently covered under Miri by the crate's unit tests.
#![cfg(all(not(miri), feature = "dynamic", feature = "macros"))]
#![allow(clippy::unwrap_used, reason = "test code")]
#![allow(clippy::missing_panics_doc, reason = "test code")]
#![allow(clippy::missing_assert_message, reason = "assertions carry a message")]
#![allow(clippy::min_ident_chars, reason = "short names in test loops")]

use bolero::TypeGenerator;
use http_path_template::{Grammar, PathTemplate};
use routerama::{DynResolver, HttpMethod, Resolver as _, Route, RouteMatch};

// The static router (oracle). The `DynResolver` built by `dyn_router` below uses
// the identical route table, so the two must agree on every request.
#[routerama::resolver(name = ApiResolver)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ApiRoute<'p> {
    #[route(GET, "/books")]
    ListBooks,
    #[route(POST, "/books")]
    CreateBook,
    #[route(GET, "/books/featured")]
    GetFeatured,
    #[route(GET, "/books/{book}")]
    GetBook { book: &'p str },
    #[route(GET, "/books/{book}/reviews/{review}")]
    GetReview { book: &'p str, review: &'p str },
    #[route(POST, "/books/{book}:archive")]
    Archive { book: &'p str },
    #[route(GET, "/files/**")]
    Files,
    #[route(GET, "/search")]
    Search,
}

fn dyn_router() -> DynResolver {
    let rule = |name, method, template| Route::new(name, method, PathTemplate::parse(template, Grammar::default()).unwrap());
    DynResolver::new([
        rule("ListBooks", HttpMethod::Get, "/books"),
        rule("CreateBook", HttpMethod::Post, "/books"),
        rule("GetFeatured", HttpMethod::Get, "/books/featured"),
        rule("GetBook", HttpMethod::Get, "/books/{book}"),
        rule("GetReview", HttpMethod::Get, "/books/{book}/reviews/{review}"),
        rule("Archive", HttpMethod::Post, "/books/{book}:archive"),
        rule("Files", HttpMethod::Get, "/files/**"),
        rule("Search", HttpMethod::Get, "/search"),
    ])
}

/// Reads a captured field from a static `ApiRoute` match by name, so a match can
/// be compared field-by-field against the dynamic router.
fn static_capture<'p>(route: ApiRoute<'p>, field: &str) -> Option<&'p str> {
    match route {
        ApiRoute::GetBook { book } | ApiRoute::Archive { book } => (field == "book").then_some(book),
        ApiRoute::GetReview { book, review } => match field {
            "book" => Some(book),
            "review" => Some(review),
            _ => None,
        },
        ApiRoute::Files | ApiRoute::ListBooks | ApiRoute::CreateBook | ApiRoute::GetFeatured | ApiRoute::Search => None,
    }
}

/// Asserts the two backends resolve `method`/`path` to the same route name and
/// the same value for every captured field. Any divergence — or a panic in
/// either backend — is a bug.
fn assert_backends_agree(router: &DynResolver, method: &str, path: &str) {
    let oracle = ApiResolver.resolve(method, path);
    let dynamic = router.resolve(method, path);

    let oracle_name = oracle.map(|route| route.name().to_owned());
    let dynamic_name = dynamic.as_ref().map(|m| m.name().to_owned());
    assert_eq!(oracle_name, dynamic_name, "name disagreement on `{method} {path}`");

    if let (Some(route), Some(matched)) = (oracle, dynamic) {
        for field in ["book", "review"] {
            assert_eq!(
                static_capture(route, field),
                matched.capture(field),
                "capture `{field}` disagreement on `{method} {path}`"
            );
        }
    }
}

/// HTTP method, biased toward the ones the table declares plus an arbitrary
/// escape hatch.
#[derive(Debug, TypeGenerator)]
enum Method {
    Get,
    Post,
    Delete,
    Put,
    Other(String),
}

impl Method {
    fn as_str(&self) -> &str {
        match self {
            Self::Get => "GET",
            Self::Post => "POST",
            Self::Delete => "DELETE",
            Self::Put => "PUT",
            Self::Other(s) => s,
        }
    }
}

// === target 1: robustness / differential over arbitrary inputs ===

/// An arbitrary method paired with an arbitrary path string.
#[derive(Debug, TypeGenerator)]
struct ArbitraryRequest {
    method: Method,
    path: String,
}

#[test]
fn resolve_never_diverges_on_arbitrary_input() {
    let router = dyn_router();
    bolero::check!().with_type::<ArbitraryRequest>().for_each(|req: &ArbitraryRequest| {
        assert_backends_agree(&router, req.method.as_str(), &req.path);
    });
}

// === target 2: differential over structured, colliding paths ===

/// A path segment drawn from the route table's alphabet, biased toward literals
/// that also appear as wildcards so structured inputs frequently collide with a
/// route. `Arbitrary` keeps the space open-ended.
#[derive(Debug, TypeGenerator)]
enum Seg {
    Books,
    Featured,
    Rust,
    Reviews,
    FortyTwo,
    Search,
    Files,
    X,
    Empty,
    Arbitrary(String),
}

impl Seg {
    fn as_str(&self) -> &str {
        match self {
            Self::Books => "books",
            Self::Featured => "featured",
            Self::Rust => "rust",
            Self::Reviews => "reviews",
            Self::FortyTwo => "42",
            Self::Search => "search",
            Self::Files => "files",
            Self::X => "x",
            Self::Empty => "",
            Self::Arbitrary(s) => s,
        }
    }
}

/// A trailing custom-verb suffix, exercising the `:verb` split.
#[derive(Debug, TypeGenerator)]
enum Verb {
    None,
    Archive,
    Other,
}

impl Verb {
    fn suffix(&self) -> &str {
        match self {
            Self::None => "",
            Self::Archive => ":archive",
            Self::Other => ":other",
        }
    }
}

/// A structured request: a method, a sequence of segments, and an optional verb.
#[derive(Debug, TypeGenerator)]
struct StructuredRequest {
    method: Method,
    segments: Vec<Seg>,
    verb: Verb,
}

impl StructuredRequest {
    /// Assembles the request path, e.g. `/books/rust:archive`. An empty segment
    /// list yields `/` so the path is always absolute.
    fn path(&self) -> String {
        let mut path = String::new();
        for seg in &self.segments {
            path.push('/');
            path.push_str(seg.as_str());
        }
        if path.is_empty() {
            path.push('/');
        }
        path.push_str(self.verb.suffix());
        path
    }
}

#[test]
fn dynamic_matches_static_on_structured_paths() {
    let router = dyn_router();
    bolero::check!()
        .with_type::<StructuredRequest>()
        .for_each(|req: &StructuredRequest| {
            assert_backends_agree(&router, req.method.as_str(), &req.path());
        });
}
