// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The build-time front door: generating a router from a `build.rs`.
//!
//! Run it with `cargo run --example build_script`.
//!
//! Most routers are simplest to define in source with the `routes!` macro (see
//! the `routing` example). Reach for the build-time generator instead when the
//! route set is computed — e.g. read from an external file such as a service
//! descriptor — at build time. In a real crate, add `routerama` as a
//! build-dependency (the `build` feature it needs is on by default):
//!
//! ```toml
//! [dependencies]
//! routerama = "0.1"
//! [build-dependencies]
//! routerama = "0.1"
//! ```
//!
//! and `build.rs` writes the generated `Route` enum into `OUT_DIR`:
//!
//! ```ignore
//! use routerama::{Generator, RouteRule, HttpMethod};
//!
//! let mut generator = Generator::new();
//! generator.add(RouteRule::new("GetBook", HttpMethod::Get, "/books/{book}".parse()?));
//! std::fs::write(out_dir.join("router.rs"), generator.generate().to_string())?;
//! ```
//!
//! The crate then pulls it in with `include!(concat!(env!("OUT_DIR"), "/router.rs"))`.
//! This example `include!`s a pre-generated equivalent (see `examples/support/`)
//! so it is self-contained; the resulting `Route` enum is used exactly like the
//! macro's.

include!("support/bookstore_router.rs");

fn main() {
    let matched = Route::resolve("GET", "/books/rust-in-action/reviews/42").expect("nested route matches");
    let Route::GetReview { book, review } = matched else {
        unreachable!("expected GetReview");
    };
    assert_eq!((book, review), ("rust-in-action", "42"));
    println!(
        "GET /books/rust-in-action/reviews/42 -> {} (book={book}, review={review})",
        matched.name()
    );

    assert!(Route::resolve("DELETE", "/books").is_none());
    println!("DELETE /books -> (no route)");
}
