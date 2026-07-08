// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Combining static and dynamic routing in one router.
//!
//! Run it with `cargo run --example hybrid_routing --features "macros dynamic"`.
//!
//! A common shape for a service is a fixed set of built-in routes plus a set of
//! routes registered at run time (plugins, tenants, config). `routerama` lets
//! you serve both through a single [`EitherRouter`]:
//!
//! - the built-in routes are a compile-time `routes!` router — zero-cost, no
//!   allocation. Alongside the enum, `routes!` also generates a zero-sized
//!   [`Router`] (`{Enum}Router`) so the static router plugs into the same trait
//!   the dynamic one uses;
//! - the run-time routes are a [`DynRouter`] built from a value.
//!
//! [`EitherRouter`] tries its primary (the static core) first and falls back to
//! the secondary (the dynamic overlay). This is the right composition when the
//! two route sets are disjoint (or the core owns a distinct path subtree), which
//! is the usual "built-ins + plugins" arrangement.

#![expect(
    clippy::literal_string_with_formatting_args,
    reason = "route path templates use `{var}` capture syntax, not string formatting"
)]

use routerama::{DynRouter, EitherRouter, HttpMethod, RouteMatch as _, RouteRule, Router as _};

// The built-in routes, known at compile time. `routes!` generates the `Api` enum
// and — because we declare a companion `struct` — a zero-sized `ApiRouter` that
// implements `Router`.
routerama::routes! {
    pub enum Api {
        ListBooks GET  "/books",
        GetBook   GET  "/books/{book}",
        Health    GET  "/health",
    }
    pub struct ApiRouter;
}

fn main() {
    // The run-time overlay: routes registered by plugins, on a disjoint subtree.
    let plugins = DynRouter::new([
        RouteRule::new("Plugin", HttpMethod::Get, "/plugins/{name}".parse().expect("valid")),
        RouteRule::new("PluginAction", HttpMethod::Post, "/plugins/{name}/{action}".parse().expect("valid")),
    ]);

    // One router serving both: the static core first, the dynamic overlay second.
    let router = EitherRouter::new(ApiRouter, plugins);

    // Built-in routes resolve through the fast static core...
    let book = router.resolve("GET", "/books/rust").expect("a match");
    assert_eq!(book.name(), "GetBook");
    assert_eq!(book.capture("book"), Some("rust"));
    assert_eq!(router.resolve("GET", "/health").expect("a match").name(), "Health");

    // ...and plugin routes fall through to the dynamic overlay, captures intact.
    let plugin = router.resolve("GET", "/plugins/auth").expect("a match");
    assert_eq!(plugin.name(), "Plugin");
    assert_eq!(plugin.capture("name"), Some("auth"));

    let action = router.resolve("POST", "/plugins/auth/enable").expect("a match");
    assert_eq!(action.name(), "PluginAction");
    assert_eq!(action.capture("name"), Some("auth"));
    assert_eq!(action.capture("action"), Some("enable"));

    // A path neither set owns is a miss.
    assert!(router.resolve("GET", "/nope").is_none());

    // The static router can also be used on its own through the same trait — or
    // directly via the enum's inherent `resolve` for zero-cost `match` dispatch.
    assert_eq!(ApiRouter.resolve("GET", "/books").expect("a match").name(), "ListBooks");
    assert!(matches!(Api::resolve("GET", "/books"), Some(Api::ListBooks)));

    println!("hybrid_routing: all assertions passed");
}
