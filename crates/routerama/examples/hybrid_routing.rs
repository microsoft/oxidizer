// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Combining static and dynamic routing in one resolver.
//!
//! Run it with `cargo run --example hybrid_routing --features "macros dynamic"`.
//!
//! A common shape for a service is a fixed set of built-in routes plus a set of
//! routes registered at run time (plugins, tenants, config). `routerama` lets
//! you serve both through a single [`EitherResolver`]:
//!
//! - the built-in routes are a compile-time `#[resolver]` resolver —
//!   zero-cost, no allocation. `#[resolver(name = ...)]` emits a zero-sized
//!   [`Resolver`] so the static resolver plugs into the same
//!   trait the dynamic one uses;
//! - the run-time routes are a [`DynResolver`] built from a value.
//!
//! [`EitherResolver`] tries its primary (the static core) first and falls back to
//! the secondary (the dynamic overlay). This is the right composition when the
//! two route sets are disjoint (or the core owns a distinct path subtree), which
//! is the usual "built-ins + plugins" arrangement.

#![expect(
    clippy::literal_string_with_formatting_args,
    reason = "route path templates use `{var}` capture syntax, not string formatting"
)]

use http_path_template::{Grammar, PathTemplate};
use routerama::{DynResolver, EitherResolver, HttpMethod, Resolver as _, Route, RouteMatch as _};

// The built-in routes, known at compile time. `#[resolver(name = ApiResolver)]`
// generates, for the `Api` enum, a zero-sized `ApiResolver` that implements
// `Resolver`.
#[routerama::resolver(name = ApiResolver)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Api<'p> {
    #[route(GET, "/books")]
    ListBooks,
    #[route(GET, "/books/{book}")]
    GetBook { book: &'p str },
    #[route(GET, "/health")]
    Health,
}

fn main() {
    // The run-time overlay: routes registered by plugins, on a disjoint subtree.
    let plugins = DynResolver::new([
        Route::new(
            "Plugin",
            HttpMethod::Get,
            PathTemplate::parse("/plugins/{name}", Grammar::default()).expect("valid"),
        ),
        Route::new(
            "PluginAction",
            HttpMethod::Post,
            PathTemplate::parse("/plugins/{name}/{action}", Grammar::default()).expect("valid"),
        ),
    ]);

    // One resolver serving both: the static core first, the dynamic overlay second.
    let resolver = EitherResolver::new(ApiResolver, plugins);

    // Built-in routes resolve through the fast static core...
    let book = resolver.resolve("GET", "/books/rust").expect("a match");
    assert_eq!(book.name(), "GetBook");
    assert_eq!(book.capture("book"), Some("rust"));
    assert_eq!(resolver.resolve("GET", "/health").expect("a match").name(), "Health");

    // ...and plugin routes fall through to the dynamic overlay, captures intact.
    let plugin = resolver.resolve("GET", "/plugins/auth").expect("a match");
    assert_eq!(plugin.name(), "Plugin");
    assert_eq!(plugin.capture("name"), Some("auth"));

    let action = resolver.resolve("POST", "/plugins/auth/enable").expect("a match");
    assert_eq!(action.name(), "PluginAction");
    assert_eq!(action.capture("name"), Some("auth"));
    assert_eq!(action.capture("action"), Some("enable"));

    // A path neither set owns is a miss.
    assert!(resolver.resolve("GET", "/nope").is_none());

    // The static resolver can also be used on its own through the same trait,
    // yielding the `Api` enum for zero-cost `match` dispatch.
    assert_eq!(ApiResolver.resolve("GET", "/books").expect("a match").name(), "ListBooks");
    assert!(matches!(ApiResolver.resolve("GET", "/books"), Some(Api::ListBooks)));

    println!("hybrid_routing: all assertions passed");
}
