// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Runtime (dynamic) routing: a router whose route set is only known at run time.
//!
//! Run it with `cargo run --example dynamic_routing --features dynamic`.
//!
//! Where the `routes!` macro and `build.rs` generator lower a *compile-time*
//! route set into a `match` (see the `routing` and `build_script` examples), a
//! [`DynRouter`] is a value built from routes discovered at run time — read from
//! config, a database, a plugin registry, or per-tenant registration. It walks
//! the *same* trie the static path lowers, so it resolves identically.
//!
//! Both routers are used through two small traits:
//! - [`Router::resolve`] maps an HTTP method + path to a match.
//! - [`RouteMatch`] exposes the matched route's [`name`](RouteMatch::name) and
//!   its [`capture`](RouteMatch::capture)d path variables, looked up by name.

#![expect(
    clippy::literal_string_with_formatting_args,
    reason = "route path templates use `{var}` capture syntax, not string formatting"
)]

use routerama::{DynRouter, HttpMethod, RouteMatch as _, RouteRule, Router as _};

fn main() {
    // Pretend this route table was just loaded from config or a database: a list
    // of `(name, method, path-template)` rows. Only at run time do we know it.
    let table = [
        ("ListBooks", HttpMethod::Get, "/books"),
        ("CreateBook", HttpMethod::Post, "/books"),
        ("GetBook", HttpMethod::Get, "/books/{book}"),
        ("GetReview", HttpMethod::Get, "/books/{book}/reviews/{review}"),
        ("Assets", HttpMethod::Get, "/assets/**"),
    ];

    // Build the router once; resolve many requests against it.
    let router = DynRouter::new(
        table
            .iter()
            .map(|(name, method, path)| RouteRule::new(*name, method.clone(), path.parse().expect("valid template"))),
    );

    // A plain match: no captured variables.
    let matched = router.resolve("GET", "/books").expect("a match");
    assert_eq!(matched.name(), "ListBooks");

    // The HTTP method is part of routing: same path, different route.
    assert_eq!(router.resolve("POST", "/books").expect("a match").name(), "CreateBook");

    // Captured path variables are read from the match by (field) name.
    let book = router.resolve("GET", "/books/rust-in-action").expect("a match");
    assert_eq!(book.name(), "GetBook");
    assert_eq!(book.capture("book"), Some("rust-in-action"));

    // A route with several captures.
    let review = router.resolve("GET", "/books/rust/reviews/42").expect("a match");
    assert_eq!(review.name(), "GetReview");
    assert_eq!(review.capture("book"), Some("rust"));
    assert_eq!(review.capture("review"), Some("42"));
    // `captures()` iterates every `(name, value)` pair.
    let pairs: Vec<_> = review.captures().collect();
    assert_eq!(pairs, [("book", "rust"), ("review", "42")]);

    // A `**` catch-all matches any (possibly empty) remainder.
    assert_eq!(router.resolve("GET", "/assets/css/site.css").expect("a match").name(), "Assets");

    // Unknown method or path: no match.
    assert!(router.resolve("DELETE", "/books").is_none());
    assert!(router.resolve("GET", "/nope").is_none());

    println!("dynamic_routing: all assertions passed");
}
