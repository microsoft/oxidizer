// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! `#[resolver]`: typed, compile-time-validated static extraction and coercion.

use std::borrow::Cow;

use routerama::{HttpMethod, ResolveError, resolver};

#[resolver]
#[derive(Debug)]
enum Route<'p> {
    #[route(GET, "/books/{book}/reviews/{review}")]
    GetReview { book: &'p str, review: u32 },

    #[route(GET, "/books/{book}")]
    GetBook { book: String },

    #[route(GET, "/files/{path=**}")]
    GetFile { path: Cow<'p, str> },

    #[route(GET, "/health")]
    Health,
}

#[test]
fn coerces_and_borrows() {
    let path = String::from("/books/rust/reviews/42");
    match route_r().resolve("GET", &path) {
        Ok(Route::GetReview { book, review }) => {
            assert_eq!(book, "rust"); // &str, zero-copy
            assert_eq!(review, 42); // u32, parsed
        }
        other => panic!("expected GetReview, got {other:?}"),
    }
}

#[test]
fn borrowed_field_is_zero_copy() {
    let path = String::from("/books/rust/reviews/42");
    match route_r().resolve("GET", &path) {
        Ok(Route::GetReview { book, .. }) => {
            assert!(std::ptr::eq(book.as_ptr(), path.as_ptr().wrapping_add("/books/".len())));
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn string_field_is_decoded() {
    let path = String::from("/books/a%20b");
    match route_r().resolve("GET", &path) {
        Ok(Route::GetBook { book }) => assert_eq!(book, "a b"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn cow_rest_field_decodes_encoded_slash() {
    let path = String::from("/files/a/b%2Fc");
    match route_r().resolve("GET", &path) {
        Ok(Route::GetFile { path }) => assert_eq!(path, "a/b/c"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn unit_route() {
    assert!(matches!(route_r().resolve("GET", "/health"), Ok(Route::Health)));
}

#[test]
fn no_match_is_not_found() {
    assert!(matches!(route_r().resolve("GET", "/nope"), Err(ResolveError::NotFound(_))));
    assert!(matches!(route_r().resolve("POST", "/health"), Err(ResolveError::NotFound(_))));
}

#[test]
fn parse_failure_is_some_err() {
    let result = route_r().resolve("GET", "/books/rust/reviews/notanumber");
    assert!(matches!(result, Err(ResolveError::InvalidCapture("review"))));
}

// An owned-only route enum remains free of synthetic lifetimes.
#[resolver]
#[derive(Debug)]
enum Owned {
    #[route(GET, "/n/{id}")]
    Num { id: u64 },
}

#[test]
fn owned_only_router_resolves() {
    match owned_r().resolve("GET", "/n/123") {
        Ok(Owned::Num { id }) => assert_eq!(id, 123),
        other => panic!("{other:?}"),
    }
}

use std::str::FromStr;

#[derive(Debug, PartialEq)]
struct BookId(u32);
impl FromStr for BookId {
    type Err = std::num::ParseIntError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(s.parse()?))
    }
}

// A dotted capture binds to the sanitized field name; custom `FromStr` works.
#[resolver]
#[derive(Debug)]
enum ShelfRoute<'p> {
    #[route(GET, "/shelves/{shelf.id}/items/{item}")]
    Get { shelf_id: BookId, item: &'p str },
}

#[test]
fn dotted_capture_and_custom_fromstr() {
    match shelf().resolve("GET", "/shelves/7/items/rust") {
        Ok(ShelfRoute::Get { shelf_id, item }) => {
            assert_eq!(shelf_id, BookId(7));
            assert_eq!(item, "rust");
        }
        other => panic!("{other:?}"),
    }
    assert!(matches!(
        shelf().resolve("GET", "/shelves/x/items/y"),
        Err(ResolveError::InvalidCapture("shelf_id"))
    ));
}

// Multiple `#[route]` on one variant resolve to the same typed variant.
#[resolver]
#[derive(Debug)]
enum MultiRoute {
    #[route(GET, "/a/{n}")]
    #[route(GET, "/b/{n}")]
    Both { n: u32 },
}

#[test]
fn multiple_routes_per_variant() {
    assert!(matches!(multi_r().resolve("GET", "/a/1"), Ok(MultiRoute::Both { n: 1 })));
    assert!(matches!(multi_r().resolve("GET", "/b/2"), Ok(MultiRoute::Both { n: 2 })));
}

// A `pub` typed resolver must not leak the hidden raw enum across a module.
mod sub {
    use routerama::resolver;
    #[resolver]
    #[derive(Debug)]
    pub(crate) enum Route<'p> {
        #[route(GET, "/x/{v}")]
        X { v: &'p str },
    }
}

#[test]
fn public_resolver_across_module_boundary() {
    assert!(matches!(
        sub::Route::resolver().resolve("GET", "/x/hi"),
        Ok(sub::Route::X { v: "hi" })
    ));
}

// A resolver may use `Cow<'p, str>` as its only borrowing field.
#[resolver]
#[derive(Debug)]
enum FileRoute<'p> {
    #[route(GET, "/files/{path=**}")]
    GetFile { path: Cow<'p, str> },
}

#[test]
fn cow_only_resolver_compiles_and_borrows() {
    let path = String::from("/files/a/b");
    match file_r().resolve("GET", &path) {
        Ok(FileRoute::GetFile { path: p }) => {
            assert_eq!(p, "a/b");
            assert!(matches!(p, Cow::Borrowed(_)), "no %-escape so it borrows");
        }
        other => panic!("{other:?}"),
    }
}

// These enums are pure-static, so each router never fails to build; the helpers
// keep the assertions terse.
fn route_r() -> RouteResolver {
    Route::resolver()
}
fn owned_r() -> OwnedResolver {
    Owned::resolver()
}
fn shelf() -> ShelfRouteResolver {
    ShelfRoute::resolver()
}
fn multi_r() -> MultiRouteResolver {
    MultiRoute::resolver()
}
fn file_r() -> FileRouteResolver {
    FileRoute::resolver()
}

#[resolver]
#[derive(Debug, PartialEq, Eq)]
enum Mixed {
    #[route(GET, "/numbers/{value}")]
    Static {
        value: u32,
    },
    Dynamic {
        value: String,
    },
}

#[test]
fn mixed_resolution_keeps_static_first_capture_error_semantics() {
    let resolver = Mixed::builder()
        .add_dynamic(HttpMethod::GET, "/numbers/{value}")
        .build()
        .expect("dynamic route is valid");

    assert_eq!(resolver.resolve("GET", "/numbers/42"), Ok(Mixed::Static { value: 42 }));
    assert_eq!(
        resolver.resolve("GET", "/numbers/not-a-number"),
        Err(ResolveError::InvalidCapture("value"))
    );
}

#[derive(Debug, PartialEq, Eq)]
struct Invariant<'p>(u32, std::marker::PhantomData<fn(&'p ()) -> &'p ()>);

impl FromStr for Invariant<'_> {
    type Err = std::num::ParseIntError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Ok(Self(value.parse()?, std::marker::PhantomData))
    }
}

#[resolver]
#[derive(Debug, PartialEq, Eq)]
enum InvariantMixed<'p> {
    #[route(GET, "/static/{value}")]
    Static {
        value: Invariant<'p>,
    },
    Dynamic {
        r#type: String,
    },
}

#[test]
fn mixed_dynamic_extractors_do_not_require_route_covariance() {
    let resolver = InvariantMixed::builder()
        .add_dynamic(HttpMethod::GET, "/dynamic/{type}")
        .build()
        .expect("the raw field name matches the unraw capture name");

    assert_eq!(
        resolver.resolve("GET", "/static/42"),
        Ok(InvariantMixed::Static {
            value: Invariant(42, std::marker::PhantomData)
        })
    );
    assert_eq!(
        resolver.resolve("GET", "/dynamic/plugin"),
        Ok(InvariantMixed::Dynamic {
            r#type: "plugin".to_owned()
        })
    );
}
