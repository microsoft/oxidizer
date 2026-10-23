// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Bolero property tests for resolver parity and safety.
//!
//! Arbitrary and structured paths must produce the same route, captures, and
//! coercion results under static and runtime resolution.
#![cfg(not(miri))]
#![allow(clippy::unwrap_used, reason = "test code")]
#![allow(clippy::missing_panics_doc, reason = "test code")]
#![allow(clippy::missing_assert_message, reason = "assertions carry a message")]
#![allow(clippy::min_ident_chars, reason = "short names in test loops")]

use bolero::TypeGenerator;
use http_path_template::{Grammar, PathTemplate};
use routerama::__rt::{RawResolver, Route, RouteMatch};
use routerama::{HttpMethod, ResolveError};

#[routerama::resolver]
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

fn dynamic_resolver() -> RawResolver {
    let rule = |name, method, template| Route::new(name, method, PathTemplate::parse(template, Grammar::default()).unwrap());
    RawResolver::new([
        rule("ListBooks", HttpMethod::GET, "/books"),
        rule("CreateBook", HttpMethod::POST, "/books"),
        rule("GetFeatured", HttpMethod::GET, "/books/featured"),
        rule("GetBook", HttpMethod::GET, "/books/{book}"),
        rule("GetReview", HttpMethod::GET, "/books/{book}/reviews/{review}"),
        rule("Archive", HttpMethod::POST, "/books/{book}:archive"),
        rule("Files", HttpMethod::GET, "/files/**"),
        rule("Search", HttpMethod::GET, "/search"),
    ])
}

/// Returns a named static capture for backend comparison.
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

/// Returns the route name used by the runtime resolver.
fn static_name(route: ApiRoute<'_>) -> &'static str {
    match route {
        ApiRoute::ListBooks => "ListBooks",
        ApiRoute::CreateBook => "CreateBook",
        ApiRoute::GetFeatured => "GetFeatured",
        ApiRoute::GetBook { .. } => "GetBook",
        ApiRoute::GetReview { .. } => "GetReview",
        ApiRoute::Archive { .. } => "Archive",
        ApiRoute::Files => "Files",
        ApiRoute::Search => "Search",
    }
}

/// Asserts that both backends return the same route and captures.
fn assert_backends_agree(resolver: &RawResolver, method: &str, path: &str) {
    let oracle = match ApiRoute::resolver().resolve(method, path) {
        Err(ResolveError::InvalidPath(_) | ResolveError::NotFound(_)) => None,
        Err(ResolveError::MissingCapture(_) | ResolveError::InvalidCapture(_) | ResolveError::UndecodableCapture(_)) => {
            unreachable!("ApiRoute has only `&str` captures")
        }
        Err(_) => unreachable!("unknown resolution error"),
        Ok(route) => Some(route),
    };
    let dynamic = resolver.resolve(method, path);

    let oracle_name = oracle.map(static_name);
    let dynamic_name = dynamic.as_ref().map(RouteMatch::name);
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

// Arbitrary-input differential.

/// An arbitrary method paired with an arbitrary path string.
#[derive(Debug, TypeGenerator)]
struct ArbitraryRequest {
    method: Method,
    path: String,
}

#[test]
fn resolve_never_diverges_on_arbitrary_input() {
    let resolver = dynamic_resolver();
    bolero::check!().with_type::<ArbitraryRequest>().for_each(|req: &ArbitraryRequest| {
        assert_backends_agree(&resolver, req.method.as_str(), &req.path);
    });
}

// Structured-path differential.

/// A segment biased toward route literals while retaining arbitrary input.
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
    let resolver = dynamic_resolver();
    bolero::check!()
        .with_type::<StructuredRequest>()
        .for_each(|req: &StructuredRequest| {
            assert_backends_agree(&resolver, req.method.as_str(), &req.path());
        });
}

// Typed static/dynamic coercion differential. Both route sets use owned fields
// because dynamic variants cannot borrow the request path.

/// Custom `FromStr` capture used by the differential.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ShelfId(u32);

impl core::str::FromStr for ShelfId {
    type Err = core::num::ParseIntError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        s.parse().map(Self)
    }
}

#[routerama::resolver]
#[derive(Debug, PartialEq, Eq)]
enum TypedStatic {
    #[route(GET, "/books")]
    ListBooks,
    #[route(GET, "/books/{book}")]
    GetBook { book: String },
    #[route(GET, "/books/{book}/reviews/{review}")]
    GetReview { book: String, review: u32 },
    #[route(GET, "/shelves/{shelf}")]
    GetShelf { shelf: ShelfId },
    #[route(GET, "/files/{path=**}")]
    GetFile { path: String },
}

#[routerama::resolver]
#[derive(Debug, PartialEq, Eq)]
enum TypedDyn {
    ListBooks,
    GetBook { book: String },
    GetReview { book: String, review: u32 },
    GetShelf { shelf: ShelfId },
    GetFile { path: String },
}

fn typed_dyn_builder() -> TypedDynResolverBuilder {
    TypedDyn::builder()
        .add_list_books(HttpMethod::GET, "/books")
        .add_get_book(HttpMethod::GET, "/books/{book}")
        .add_get_review(HttpMethod::GET, "/books/{book}/reviews/{review}")
        .add_get_shelf(HttpMethod::GET, "/shelves/{shelf}")
        .add_get_file(HttpMethod::GET, "/files/{path=**}")
}

fn build_typed_dyn() -> TypedDynResolver {
    typed_dyn_builder()
        .build()
        .expect("every dynamic route registers with matching captures")
}

#[test]
fn dynamic_capture_permutation_follows_variant_field_order() {
    let resolver = typed_dyn_builder()
        .add_get_review(HttpMethod::GET, "/reviews/{review}/books/{book}")
        .build()
        .expect("permuted captures are valid");
    assert_eq!(
        resolver.resolve("GET", "/reviews/42/books/rust"),
        Ok(TypedDyn::GetReview {
            book: "rust".to_owned(),
            review: 42,
        })
    );
}

/// The normalized result of resolving a request, so the two typed backends can
/// be compared regardless of their distinct enum types.
#[derive(Debug, PartialEq, Eq)]
enum Outcome {
    NotFound,
    Error(CaptureOutcome),
    Match {
        name: &'static str,
        fields: Vec<(&'static str, String)>,
    },
}

#[derive(Debug, PartialEq, Eq)]
enum CaptureOutcome {
    InvalidPath,
    Missing(&'static str),
    Invalid(&'static str),
    Undecodable(&'static str),
}

fn capture_outcome(error: ResolveError<'_>) -> CaptureOutcome {
    match error {
        ResolveError::InvalidPath(_) => CaptureOutcome::InvalidPath,
        ResolveError::MissingCapture(field) => CaptureOutcome::Missing(field),
        ResolveError::InvalidCapture(field) => CaptureOutcome::Invalid(field),
        ResolveError::UndecodableCapture(field) => CaptureOutcome::Undecodable(field),
        ResolveError::NotFound(_) => unreachable!("not-found errors are handled separately"),
        _ => unreachable!("unknown resolution error"),
    }
}

fn static_outcome(route: Result<TypedStatic, ResolveError<'_>>) -> Outcome {
    match route {
        Ok(TypedStatic::ListBooks) => Outcome::Match {
            name: "ListBooks",
            fields: Vec::new(),
        },
        Ok(TypedStatic::GetBook { book }) => Outcome::Match {
            name: "GetBook",
            fields: vec![("book", book)],
        },
        Ok(TypedStatic::GetReview { book, review }) => Outcome::Match {
            name: "GetReview",
            fields: vec![("book", book), ("review", review.to_string())],
        },
        Ok(TypedStatic::GetShelf { shelf }) => Outcome::Match {
            name: "GetShelf",
            fields: vec![("shelf", shelf.0.to_string())],
        },
        Ok(TypedStatic::GetFile { path }) => Outcome::Match {
            name: "GetFile",
            fields: vec![("path", path)],
        },
        Err(ResolveError::NotFound(_)) => Outcome::NotFound,
        Err(error) => Outcome::Error(capture_outcome(error)),
    }
}

fn dynamic_outcome(route: Result<TypedDyn, ResolveError<'_>>) -> Outcome {
    match route {
        Ok(TypedDyn::ListBooks) => Outcome::Match {
            name: "ListBooks",
            fields: Vec::new(),
        },
        Ok(TypedDyn::GetBook { book }) => Outcome::Match {
            name: "GetBook",
            fields: vec![("book", book)],
        },
        Ok(TypedDyn::GetReview { book, review }) => Outcome::Match {
            name: "GetReview",
            fields: vec![("book", book), ("review", review.to_string())],
        },
        Ok(TypedDyn::GetShelf { shelf }) => Outcome::Match {
            name: "GetShelf",
            fields: vec![("shelf", shelf.0.to_string())],
        },
        Ok(TypedDyn::GetFile { path }) => Outcome::Match {
            name: "GetFile",
            fields: vec![("path", path)],
        },
        Err(ResolveError::NotFound(_)) => Outcome::NotFound,
        Err(error) => Outcome::Error(capture_outcome(error)),
    }
}

/// A path segment biased toward values that exercise coercion: valid and
/// overflowing numbers, well-formed and malformed percent escapes, invalid-UTF-8
/// escapes, plus the structural literals that let a request reach a capture.
#[derive(Debug, TypeGenerator)]
enum TypedSeg {
    Books,
    Reviews,
    Shelves,
    Files,
    SmallNum,
    Zero,
    BigNum,
    Encoded,
    Multibyte,
    BadEscape,
    Truncated,
    HighByte,
    Free(String),
}

impl TypedSeg {
    fn as_str(&self) -> &str {
        match self {
            Self::Books => "books",
            Self::Reviews => "reviews",
            Self::Shelves => "shelves",
            Self::Files => "files",
            Self::SmallNum => "42",
            Self::Zero => "0",
            Self::BigNum => "99999999999999999999", // overflows u32 -> Parse error
            Self::Encoded => "a%20b",               // decodes to "a b"
            Self::Multibyte => "%E2%9C%93",         // decodes to "✓"
            Self::BadEscape => "%zz",               // malformed -> Decode error
            Self::Truncated => "%2",                // truncated escape -> Decode error
            Self::HighByte => "%FF",                // invalid UTF-8 -> Decode error
            Self::Free(s) => s,
        }
    }
}

/// A structured typed request: a method plus a sequence of coercion-flavored
/// segments assembled into an absolute path.
#[derive(Debug, TypeGenerator)]
struct TypedRequest {
    method: Method,
    segments: Vec<TypedSeg>,
}

impl TypedRequest {
    fn path(&self) -> String {
        let mut path = String::new();
        for seg in &self.segments {
            path.push('/');
            path.push_str(seg.as_str());
        }
        if path.is_empty() {
            path.push('/');
        }
        path
    }
}

#[test]
fn typed_static_and_dynamic_agree_on_coercion() {
    let static_resolver = TypedStatic::resolver();
    let dynamic_resolver = build_typed_dyn();
    bolero::check!().with_type::<TypedRequest>().for_each(|req: &TypedRequest| {
        let method = req.method.as_str();
        let path = req.path();
        let expected = static_outcome(static_resolver.resolve(method, &path));
        let actual = dynamic_outcome(dynamic_resolver.resolve(method, &path));
        assert_eq!(expected, actual, "typed coercion disagreement on `{method} {path}`");
    });
}
