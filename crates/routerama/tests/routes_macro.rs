// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Behavioral tests for the `#[resolver]` in-source front door, mirroring
//! the cases the build-time generator is tested against: unit routes, captured
//! variables (including dotted field paths), custom verbs, method
//! disambiguation, and misses. Runs only when the `macros` feature is enabled.

#![cfg(feature = "macros")]

use routerama::{Resolver as _, RouteMatch as _, resolver};

// A single route enum with its named zero-sized resolver.
#[resolver(name = ApiResolver)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ApiRoute<'p> {
    #[route(GET, "/books")]
    ListBooks,
    #[route(POST, "/books")]
    CreateBook,
    #[route(GET, "/books/{book}")]
    GetBook { book: &'p str },
    #[route(GET, "/books/{book}/reviews/{review}")]
    GetReview { book: &'p str, review: &'p str },
    #[route(GET, "/items/{item.id}")]
    GetItem { item_id: &'p str },
    #[route(POST, "/books/{book}:archive")]
    ArchiveBook { book: &'p str },
    #[route(GET, "/search")]
    SearchBooks,
}

#[test]
fn unit_route_has_no_captures() {
    assert_eq!(ApiResolver.resolve("GET", "/books"), Some(ApiRoute::ListBooks));
}

#[test]
fn method_disambiguates_same_path() {
    assert_eq!(ApiResolver.resolve("GET", "/books"), Some(ApiRoute::ListBooks));
    assert_eq!(ApiResolver.resolve("POST", "/books"), Some(ApiRoute::CreateBook));
}

#[test]
fn single_capture_is_read_from_the_variant_field() {
    assert_eq!(ApiResolver.resolve("GET", "/books/rust"), Some(ApiRoute::GetBook { book: "rust" }));
}

#[test]
fn multiple_captures() {
    assert_eq!(
        ApiResolver.resolve("GET", "/books/rust/reviews/42"),
        Some(ApiRoute::GetReview {
            book: "rust",
            review: "42"
        })
    );
}

#[test]
fn dotted_capture_becomes_an_underscore_field() {
    // `{item.id}` maps to the `item_id` field.
    assert_eq!(ApiResolver.resolve("GET", "/items/99"), Some(ApiRoute::GetItem { item_id: "99" }));
}

#[test]
fn custom_verb_selects_a_distinct_route() {
    assert_eq!(
        ApiResolver.resolve("POST", "/books/rust:archive"),
        Some(ApiRoute::ArchiveBook { book: "rust" })
    );
    // The verb is required: the same method+path without it does not match.
    assert_eq!(ApiResolver.resolve("POST", "/books/rust"), None);
}

#[test]
fn misses_resolve_to_none() {
    assert_eq!(ApiResolver.resolve("DELETE", "/books"), None);
    assert_eq!(ApiResolver.resolve("GET", "/unknown"), None);
    assert_eq!(ApiResolver.resolve("GET", "/books/rust/extra"), None);
}

#[test]
fn name_recovers_the_route_name() {
    let matched = ApiResolver.resolve("GET", "/search").expect("match");
    assert_eq!(matched.name(), "SearchBooks");
}

// A second resolver in the same scope: possible because each is a distinct named
// zero-sized resolver type.
#[resolver(name = UserResolver)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum UserRoute<'p> {
    #[route(GET, "/users")]
    ListUsers,
    #[route(GET, "/users/{user}")]
    GetUser { user: &'p str },
}

#[test]
fn two_resolvers_coexist_in_the_same_scope() {
    assert_eq!(UserResolver.resolve("GET", "/users/ada"), Some(UserRoute::GetUser { user: "ada" }));
    // The two resolvers are independent: a book path does not match the user one.
    assert_eq!(UserResolver.resolve("GET", "/books/rust"), None);
    assert_eq!(ApiResolver.resolve("GET", "/users/ada"), None);
}

// Exercises the remaining HTTP-method tokens (PUT, DELETE, PATCH) and a custom
// verb (PURGE becomes `HttpMethod::Custom`).
#[resolver(name = MethodResolver)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MethodRoute<'p> {
    #[route(PUT, "/items/{item}")]
    Replace { item: &'p str },
    #[route(DELETE, "/items/{item}")]
    Remove { item: &'p str },
    #[route(PATCH, "/items/{item}")]
    Tweak { item: &'p str },
    #[route(PURGE, "/items/{item}")]
    Purge { item: &'p str },
}

#[test]
fn every_method_token_resolves_its_route() {
    assert_eq!(MethodResolver.resolve("PUT", "/items/1"), Some(MethodRoute::Replace { item: "1" }));
    assert_eq!(
        MethodResolver.resolve("DELETE", "/items/1"),
        Some(MethodRoute::Remove { item: "1" })
    );
    assert_eq!(MethodResolver.resolve("PATCH", "/items/1"), Some(MethodRoute::Tweak { item: "1" }));
    assert_eq!(MethodResolver.resolve("PURGE", "/items/1"), Some(MethodRoute::Purge { item: "1" }));
}

// Regression: a captured field named exactly `name` must not shadow the
// parameter of the generated `RouteMatch::capture(&self, key)` method. This only
// manifests with `#[resolver(name = ...)]` (so the `RouteMatch` impl, and thus
// `capture`, is generated) and a `{name}` capture — a very common path variable.
#[resolver(name = NameResolver)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum NameRoute<'p> {
    #[route(GET, "/users/{name}")]
    GetUser { name: &'p str },
    #[route(GET, "/tags/{name}/{value}")]
    GetTag { name: &'p str, value: &'p str },
}

#[test]
fn captured_field_named_name_does_not_shadow_the_capture_key() {
    use routerama::{Resolver as _, RouteMatch as _};

    let user = NameResolver.resolve("GET", "/users/ada").expect("match");
    assert_eq!(user.name(), "GetUser");
    // The requested key `"name"` returns the captured value, not something else.
    assert_eq!(user.capture("name"), Some("ada"));
    assert_eq!(user.capture("value"), None);
    assert_eq!(user.capture("missing"), None);

    // `name` alongside another capture in the same variant still keys correctly.
    let tag = NameResolver.resolve("GET", "/tags/rust/1.0").expect("match");
    assert_eq!(tag.name(), "GetTag");
    assert_eq!(tag.capture("name"), Some("rust"));
    assert_eq!(tag.capture("value"), Some("1.0"));
}

// A captured field named exactly like the generated `capture` key parameter
// (`__key`) must not shadow it: the generated variant fields are bound under a
// `__cap_` prefix.
#[resolver(name = KeyResolver)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum KeyRoute<'p> {
    #[route(GET, "/things/{__key}")]
    GetThing { __key: &'p str },
}

#[test]
fn captured_field_named_like_the_capture_key_parameter_resolves() {
    use routerama::{Resolver as _, RouteMatch as _};

    let thing = KeyResolver.resolve("GET", "/things/widget").expect("match");
    assert_eq!(thing.name(), "GetThing");
    assert_eq!(thing.capture("__key"), Some("widget"));
    assert_eq!(thing.capture("missing"), None);
}

// A variant may carry more than one `#[route]`, binding the same name to several
// method/path pairs (each must capture the same variables).
#[resolver(name = MultiResolver)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MultiRoute<'p> {
    // Same path, two methods, sharing one unit variant.
    #[route(GET, "/status")]
    #[route(HEAD, "/status")]
    Status,
    // Two different paths capturing the same variable, sharing one variant.
    #[route(GET, "/users/{user}")]
    #[route(GET, "/accounts/{user}")]
    User { user: &'p str },
}

#[test]
fn multiple_route_attributes_bind_one_variant_to_each() {
    use routerama::{Resolver as _, RouteMatch as _};

    // Both methods reach the shared unit variant.
    assert_eq!(MultiResolver.resolve("GET", "/status"), Some(MultiRoute::Status));
    assert_eq!(MultiResolver.resolve("HEAD", "/status"), Some(MultiRoute::Status));
    assert!(MultiResolver.resolve("POST", "/status").is_none());

    // Both paths reach the shared capturing variant, each binding its own value.
    assert_eq!(
        MultiResolver.resolve("GET", "/users/alice"),
        Some(MultiRoute::User { user: "alice" })
    );
    assert_eq!(
        MultiResolver.resolve("GET", "/accounts/bob"),
        Some(MultiRoute::User { user: "bob" })
    );
    let matched = MultiResolver.resolve("GET", "/accounts/bob").expect("match");
    assert_eq!(matched.capture("user"), Some("bob"));
}
