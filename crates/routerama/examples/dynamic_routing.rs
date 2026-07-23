// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Runtime (dynamic) routing: a resolver whose paths are only known at run time.
//!
//! Run it with `cargo run --example dynamic_routing`.
//!
//! Static routes put `#[route]` on enum variants and bake their paths into the
//! resolver. Dynamic variants omit `#[route]`: they name the typed outcomes your
//! service can handle, while the generated builder registers their method + path
//! templates at run time from config, a database, a plugin registry, or
//! per-tenant setup.
//!
//! The resolver is still a typed value: resolving a request returns the enum
//! variant directly, with captures already coerced into owned fields.

#![allow(
    clippy::literal_string_with_formatting_args,
    reason = "route path templates use `{var}` capture syntax, not string formatting"
)]

use routerama::{HttpMethod, ResolveError, resolver};

#[resolver]
#[derive(Debug, PartialEq, Eq)]
enum RuntimeRoute {
    ListBooks,
    CreateBook,
    GetBook { book: String },
    GetReview { book: String, review: u32 },
    Assets { path: String },
}

fn main() {
    let resolver = RuntimeRoute::builder()
        .add_list_books(HttpMethod::GET, "/books")
        .add_create_book(HttpMethod::POST, "/books")
        .add_get_book(HttpMethod::GET, "/books/{book}")
        .add_get_review(HttpMethod::GET, "/books/{book}/reviews/{review}")
        .add_assets(HttpMethod::GET, "/assets/{path=**}")
        .build()
        .expect("all dynamic routes are registered with matching captures");

    assert_eq!(resolver.resolve("GET", "/books"), Ok(RuntimeRoute::ListBooks));

    assert_eq!(resolver.resolve("POST", "/books"), Ok(RuntimeRoute::CreateBook));

    assert_eq!(
        resolver.resolve("GET", "/books/rust-in-action"),
        Ok(RuntimeRoute::GetBook {
            book: "rust-in-action".to_owned(),
        })
    );

    assert_eq!(
        resolver.resolve("GET", "/books/rust/reviews/42"),
        Ok(RuntimeRoute::GetReview {
            book: "rust".to_owned(),
            review: 42,
        })
    );

    assert_eq!(
        resolver.resolve("GET", "/assets/css/site.css"),
        Ok(RuntimeRoute::Assets {
            path: "css/site.css".to_owned(),
        })
    );

    assert_eq!(
        resolver.resolve("GET", "/books/rust/reviews/not-a-number"),
        Err(ResolveError::InvalidCapture("review"))
    );

    assert_eq!(resolver.resolve("DELETE", "/books"), Err(ResolveError::NotFound("/books")));
    assert_eq!(resolver.resolve("GET", "/nope"), Err(ResolveError::NotFound("/nope")));

    let action = match resolver.resolve("GET", "/books/rust/reviews/42") {
        Ok(RuntimeRoute::ListBooks) => "list all books".to_owned(),
        Ok(RuntimeRoute::CreateBook) => "create a book".to_owned(),
        Ok(RuntimeRoute::GetBook { book }) => format!("get book {book}"),
        Ok(RuntimeRoute::GetReview { book, review }) => format!("review {review} of book {book}"),
        Ok(RuntimeRoute::Assets { path }) => format!("serve asset {path}"),
        Err(error @ (ResolveError::MissingCapture(_) | ResolveError::InvalidCapture(_) | ResolveError::UndecodableCapture(_))) => {
            format!("400: {error}")
        }
        Err(ResolveError::NotFound(_)) => "404".to_owned(),
        Err(error) => format!("routing error: {error}"),
    };
    assert_eq!(action, "review 42 of book rust");

    println!("dynamic_routing: all assertions passed");
}
