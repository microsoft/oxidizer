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
#![allow(
    clippy::redundant_field_names,
    reason = "the wide-table enum is declared through a macro, so the resolver macro's generated field initializers are attributed to it"
)]

use bolero::TypeGenerator;
use http_path_template::{Grammar, PathTemplate};
use routerama::resolve::__private::{RawResolver, Route, RouteMatch};
use routerama::resolve::{HttpMethod, ResolveError};

#[routerama::resolve::resolver]
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

#[routerama::resolve::resolver]
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

#[routerama::resolve::resolver]
#[derive(Debug, PartialEq, Eq)]
enum TypedDyn {
    #[route(dynamic)]
    ListBooks,
    #[route(dynamic)]
    GetBook { book: String },
    #[route(dynamic)]
    GetReview { book: String, review: u32 },
    #[route(dynamic)]
    GetShelf { shelf: ShelfId },
    #[route(dynamic)]
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

// Wide-fanout differential. A node carrying far more sibling literals than the
// runtime matcher's linear-scan threshold must resolve exactly like the
// generated static router, including the affix, single-wildcard, and rest
// siblings that share the node.

macro_rules! wide_routes {
    ($($variant:ident => $literal:tt),+ $(,)?) => {
        #[routerama::resolve::resolver]
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        enum WideRoute<'p> {
            $(
                #[route(GET, $literal)]
                $variant,
            )+
            #[route(GET, "/wide/img-{id}.png")]
            Image { id: &'p str },
            #[route(GET, "/wide/{name}")]
            Single { name: &'p str },
            #[route(GET, "/wide/{tail=**}")]
            Rest { tail: &'p str },
        }

        /// The same route set, registered at run time.
        fn wide_dynamic_resolver() -> RawResolver {
            let rule = |name: &str, template: &str| {
                Route::new(name, "GET", PathTemplate::parse(template, Grammar::default().with_segment_affixes()).unwrap())
            };
            RawResolver::new([
                $( rule(stringify!($variant), $literal), )+
                rule("Image", "/wide/img-{id}.png"),
                rule("Single", "/wide/{name}"),
                rule("Rest", "/wide/{tail=**}"),
            ])
        }

        fn wide_static_name(route: WideRoute<'_>) -> &'static str {
            match route {
                $( WideRoute::$variant => stringify!($variant), )+
                WideRoute::Image { .. } => "Image",
                WideRoute::Single { .. } => "Single",
                WideRoute::Rest { .. } => "Rest",
            }
        }
    };
}

wide_routes! {
    K00 => "/wide/k00",
    K01 => "/wide/k01",
    K02 => "/wide/k02",
    K03 => "/wide/k03",
    K04 => "/wide/k04",
    K05 => "/wide/k05",
    K06 => "/wide/k06",
    K07 => "/wide/k07",
    K08 => "/wide/k08",
    K09 => "/wide/k09",
    K10 => "/wide/k10",
    K11 => "/wide/k11",
    K12 => "/wide/k12",
    K13 => "/wide/k13",
    K14 => "/wide/k14",
    K15 => "/wide/k15",
    K16 => "/wide/k16",
    K17 => "/wide/k17",
    K18 => "/wide/k18",
    K19 => "/wide/k19",
    K20 => "/wide/k20",
    K21 => "/wide/k21",
    K22 => "/wide/k22",
    K23 => "/wide/k23",
    K24 => "/wide/k24",
    K25 => "/wide/k25",
    K26 => "/wide/k26",
    K27 => "/wide/k27",
    K28 => "/wide/k28",
    K29 => "/wide/k29",
    K30 => "/wide/k30",
    K31 => "/wide/k31",
    K32 => "/wide/k32",
    K33 => "/wide/k33",
    K34 => "/wide/k34",
    K35 => "/wide/k35",
    K36 => "/wide/k36",
    K37 => "/wide/k37",
    K38 => "/wide/k38",
    K39 => "/wide/k39",
}

/// Returns a named capture of a wide-table match.
fn wide_static_capture<'p>(route: WideRoute<'p>, field: &str) -> Option<&'p str> {
    match route {
        WideRoute::Image { id } => (field == "id").then_some(id),
        WideRoute::Single { name } => (field == "name").then_some(name),
        WideRoute::Rest { tail } => (field == "tail").then_some(tail),
        _ => None,
    }
}

/// A segment biased toward the wide table's keys, its affix shape, and the
/// boundaries between them, while retaining arbitrary input.
#[derive(Debug, TypeGenerator)]
enum WideSeg {
    /// `k00`..`k47`: the registered keys plus eight neighbouring misses.
    Key(u8),
    /// `img-N.png`, which only the affix sibling matches.
    Image(u8),
    /// `img-.png`, whose empty middle the affix guard rejects.
    ImageEmpty,
    /// A key prefix (`k0`) or extension (`k000`), which must not match.
    Truncated,
    Extended,
    Arbitrary(String),
}

impl WideSeg {
    fn to_segment(&self) -> String {
        match self {
            Self::Key(value) => format!("k{:02}", value % 48),
            Self::Image(value) => format!("img-{value}.png"),
            Self::ImageEmpty => "img-.png".to_string(),
            Self::Truncated => "k0".to_string(),
            Self::Extended => "k000".to_string(),
            Self::Arbitrary(value) => value.clone(),
        }
    }
}

/// A request against the wide table: a method, the wide node's segment, and
/// arbitrary trailing segments so the rest sibling is reachable.
#[derive(Debug, TypeGenerator)]
struct WideRequest {
    method: Method,
    segment: WideSeg,
    tail: Vec<Seg>,
}

impl WideRequest {
    fn path(&self) -> String {
        let mut path = format!("/wide/{}", self.segment.to_segment());
        for seg in &self.tail {
            path.push('/');
            path.push_str(seg.as_str());
        }
        path
    }
}

#[test]
fn wide_literal_fanout_matches_the_static_router() {
    let resolver = wide_dynamic_resolver();
    bolero::check!().with_type::<WideRequest>().for_each(|req: &WideRequest| {
        let method = req.method.as_str();
        let path = req.path();
        let oracle = match WideRoute::resolver().resolve(method, &path) {
            Err(ResolveError::InvalidPath(_) | ResolveError::NotFound(_)) => None,
            Err(_) => unreachable!("WideRoute has only `&str` captures"),
            Ok(route) => Some(route),
        };
        let dynamic = resolver.resolve(method, &path);

        assert_eq!(
            oracle.map(wide_static_name),
            dynamic.as_ref().map(RouteMatch::name),
            "name disagreement on `{method} {path}`"
        );
        if let (Some(route), Some(matched)) = (oracle, dynamic) {
            for field in ["id", "name", "tail"] {
                assert_eq!(
                    wide_static_capture(route, field),
                    matched.capture(field),
                    "capture `{field}` disagreement on `{method} {path}`"
                );
            }
        }
    });
}
