// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Behavioral tests for the `routes!` in-source front door, mirroring the cases
//! the build-time generator is tested against: unit routes, captured variables
//! (including dotted field paths), custom verbs, method disambiguation, and
//! misses. Runs only when the `macros` feature is enabled.

#![cfg(feature = "macros")]

// A single named router with an inherent `resolve` associated function.
// Entries use the preferred comma separator.
routerama::routes! {
    pub enum ApiRoute {
        ListBooks   GET    "/books",
        CreateBook  POST   "/books",
        GetBook     GET    "/books/{book}",
        GetReview   GET    "/books/{book}/reviews/{review}",
        GetItem     GET    "/items/{item.id}",
        ArchiveBook POST   "/books/{book}:archive",
        SearchBooks GET    "/search",
    }
}

#[test]
fn unit_route_has_no_captures() {
    assert_eq!(ApiRoute::resolve("GET", "/books"), Some(ApiRoute::ListBooks));
}

#[test]
fn method_disambiguates_same_path() {
    assert_eq!(ApiRoute::resolve("GET", "/books"), Some(ApiRoute::ListBooks));
    assert_eq!(ApiRoute::resolve("POST", "/books"), Some(ApiRoute::CreateBook));
}

#[test]
fn single_capture_is_read_from_the_variant_field() {
    assert_eq!(ApiRoute::resolve("GET", "/books/rust"), Some(ApiRoute::GetBook { book: "rust" }));
}

#[test]
fn multiple_captures() {
    assert_eq!(
        ApiRoute::resolve("GET", "/books/rust/reviews/42"),
        Some(ApiRoute::GetReview {
            book: "rust",
            review: "42"
        })
    );
}

#[test]
fn dotted_capture_becomes_an_underscore_field() {
    // `{item.id}` maps to the `item_id` field.
    assert_eq!(ApiRoute::resolve("GET", "/items/99"), Some(ApiRoute::GetItem { item_id: "99" }));
}

#[test]
fn custom_verb_selects_a_distinct_route() {
    assert_eq!(
        ApiRoute::resolve("POST", "/books/rust:archive"),
        Some(ApiRoute::ArchiveBook { book: "rust" })
    );
    // The verb is required: the same method+path without it does not match.
    assert_eq!(ApiRoute::resolve("POST", "/books/rust"), None);
}

#[test]
fn misses_resolve_to_none() {
    assert_eq!(ApiRoute::resolve("DELETE", "/books"), None);
    assert_eq!(ApiRoute::resolve("GET", "/unknown"), None);
    assert_eq!(ApiRoute::resolve("GET", "/books/rust/extra"), None);
}

#[test]
fn name_recovers_the_route_name() {
    assert_eq!(ApiRoute::resolve("GET", "/search").map(|r| r.name()), Some("SearchBooks"));
}

// A second router in the same scope: possible only because each is a distinct
// named type with its own inherent `resolve` (a single generated symbol). This
// one keeps the legacy `;` separator, exercising backward compatibility.
routerama::routes! {
    enum UserRoute {
        ListUsers GET "/users",
        GetUser   GET "/users/{user}"
    }
}

#[test]
fn two_routers_coexist_in_the_same_scope() {
    assert_eq!(UserRoute::resolve("GET", "/users/ada"), Some(UserRoute::GetUser { user: "ada" }));
    // The two routers are independent: a book path does not match the user router.
    assert_eq!(UserRoute::resolve("GET", "/books/rust"), None);
    assert_eq!(ApiRoute::resolve("GET", "/users/ada"), None);
}

// Exercises the remaining HTTP-method tokens (PUT, DELETE, PATCH) and a custom
// verb, plus a final entry with no trailing `,` (the separator is optional).
routerama::routes! {
    enum MethodRoute {
        Replace PUT    "/items/{item}",
        Remove  DELETE "/items/{item}",
        Tweak   PATCH  "/items/{item}",
        Purge   PURGE  "/items/{item}"
    }
}

#[test]
fn every_method_token_resolves_its_route() {
    assert_eq!(MethodRoute::resolve("PUT", "/items/1"), Some(MethodRoute::Replace { item: "1" }));
    assert_eq!(MethodRoute::resolve("DELETE", "/items/1"), Some(MethodRoute::Remove { item: "1" }));
    assert_eq!(MethodRoute::resolve("PATCH", "/items/1"), Some(MethodRoute::Tweak { item: "1" }));
    // A custom (non-standard) method token becomes `HttpMethod::Custom`.
    assert_eq!(MethodRoute::resolve("PURGE", "/items/1"), Some(MethodRoute::Purge { item: "1" }));
}
