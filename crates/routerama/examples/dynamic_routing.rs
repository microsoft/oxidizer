// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Runtime (dynamic) routing: a resolver whose route set is only known at run time.
//!
//! Run it with `cargo run --example dynamic_routing --features dynamic`.
//!
//! Where `#[resolver]` and the `build.rs` generator lower a *compile-time*
//! route set into a `match` (see the `routing` and `build_script` examples), a
//! [`DynResolver`] is a value built from routes discovered at run time — read from
//! config, a database, a plugin registry, or per-tenant registration. It walks
//! the *same* trie the static path lowers, so it resolves identically.
//!
//! Both routers are used through two small traits:
//! - [`Resolver::resolve`] maps an HTTP method + path to a match.
//! - [`RouteMatch`] exposes the matched route's [`name`](RouteMatch::name) and
//!   its [`capture`](RouteMatch::capture)d path variables, looked up by name.
//!
//! The example first dispatches by matching on the route name, then shows the
//! recommended alternative: attach a handler to each route with
//! [`DynResolver::with_values`] and invoke the one a match returns via
//! [`DynMatch::value`](routerama::DynMatch::value) — no by-name lookup.

#![expect(
    clippy::literal_string_with_formatting_args,
    reason = "route path templates use `{var}` capture syntax, not string formatting"
)]

use http_path_template::{Grammar, PathTemplate};
use routerama::{DynResolver, HttpMethod, Resolver as _, Route, RouteMatch};

fn main() {
    type Handler = fn(&dyn RouteMatch<'_>) -> String;

    // Pretend this route table was just loaded from config or a database: a list
    // of `(name, method, path-template)` rows. Only at run time do we know it.
    let table = [
        ("ListBooks", HttpMethod::Get, "/books"),
        ("CreateBook", HttpMethod::Post, "/books"),
        ("GetBook", HttpMethod::Get, "/books/{book}"),
        ("GetReview", HttpMethod::Get, "/books/{book}/reviews/{review}"),
        ("Assets", HttpMethod::Get, "/assets/**"),
    ];

    // Build the resolver once; resolve many requests against it.
    let resolver = DynResolver::new(table.iter().map(|(name, method, path)| {
        Route::new(
            *name,
            method.clone(),
            PathTemplate::parse(path, Grammar::default()).expect("valid template"),
        )
    }));

    // A plain match: no captured variables.
    let matched = resolver.resolve("GET", "/books").expect("a match");
    assert_eq!(matched.name(), "ListBooks");

    // The HTTP method is part of routing: same path, different route.
    assert_eq!(resolver.resolve("POST", "/books").expect("a match").name(), "CreateBook");

    // Captured path variables are read from the match by (field) name.
    let book = resolver.resolve("GET", "/books/rust-in-action").expect("a match");
    assert_eq!(book.name(), "GetBook");
    assert_eq!(book.capture("book"), Some("rust-in-action"));

    // A route with several captures.
    let review = resolver.resolve("GET", "/books/rust/reviews/42").expect("a match");
    assert_eq!(review.name(), "GetReview");
    assert_eq!(review.capture("book"), Some("rust"));
    assert_eq!(review.capture("review"), Some("42"));
    // `captures()` iterates every `(name, value)` pair.
    let pairs: Vec<_> = review.captures().collect();
    assert_eq!(pairs, [("book", "rust"), ("review", "42")]);

    // A `**` catch-all matches any (possibly empty) remainder.
    assert_eq!(resolver.resolve("GET", "/assets/css/site.css").expect("a match").name(), "Assets");

    // Unknown method or path: no match.
    assert!(resolver.resolve("DELETE", "/books").is_none());
    assert!(resolver.resolve("GET", "/nope").is_none());

    // --- Attaching handlers: dispatch without a by-name lookup. ---
    //
    // Rather than matching on the route name, attach a value — here a handler — to
    // each route with `with_values`. A match hands the handler straight back via
    // `value()`, so you invoke it directly. Every handler shares one signature and
    // pulls the captures it needs from the match, so routes with different
    // variables coexist in one table.
    let handlers: [(&str, HttpMethod, &str, Handler); 3] = [
        ("ListBooks", HttpMethod::Get, "/books", |_m| "list all books".to_owned()),
        ("GetBook", HttpMethod::Get, "/books/{book}", |m| {
            format!("get book {}", m.capture("book").unwrap_or("?"))
        }),
        ("GetReview", HttpMethod::Get, "/books/{book}/reviews/{review}", |m| {
            format!(
                "review {} of book {}",
                m.capture("review").unwrap_or("?"),
                m.capture("book").unwrap_or("?")
            )
        }),
    ];
    let app = DynResolver::with_values(handlers.iter().map(|(name, method, path, handler)| {
        (
            Route::new(
                *name,
                method.clone(),
                PathTemplate::parse(path, Grammar::default()).expect("valid template"),
            ),
            *handler,
        )
    }));

    let matched = app.resolve("GET", "/books/rust/reviews/42").expect("a match");
    let handler = *matched.value();
    assert_eq!(handler(&matched), "review 42 of book rust");

    println!("dynamic_routing: all assertions passed");
}
