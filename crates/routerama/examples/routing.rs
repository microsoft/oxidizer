// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! A tour of `routerama` using in-source `#[resolver]` — the simplest way
//! to define a resolver: no `build.rs`, no generated file.
//!
//! Run it with `cargo run --example routing --features macros`.
//!
//! `#[resolver]` attaches routing to a normal enum, generating a zero-sized
//! resolver whose `resolve(method, path)` returns the enum directly — a `Copy`
//! value whose variant carries any captured path variables as `&str` fields
//! borrowed from the request path, with no allocation. Because each generated
//! resolver is a distinct named type, several can coexist in one scope.
//!
//! For the build-time alternative — generating the resolver from a `build.rs` when
//! the route set comes from an external file — see the `build_script` example.

#[routerama::resolver(name = BookResolver)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BookRoute<'p> {
    #[route(GET, "/books")]
    ListBooks,

    #[route(POST, "/books")]
    CreateBook,
    #[route(GET, "/books/{book}")]
    GetBook { book: &'p str },
    #[route(GET, "/books/{book}/reviews/{review}")]
    GetReview { book: &'p str, review: &'p str },
    #[route(POST, "/books/{book}:archive")]
    ArchiveBook { book: &'p str },
    #[route(GET, "/books/{book}/cover-{size}.png")]
    GetCover { book: &'p str, size: &'p str },
    #[route(GET, "/search")]
    SearchBooks,
}

fn main() {
    use routerama::Resolver as _;
    // A unit variant: the route captures no path variables.
    assert_eq!(BookResolver.resolve("GET", "/books"), Some(BookRoute::ListBooks));

    // The HTTP method is part of routing: same path, different route.
    assert_eq!(BookResolver.resolve("POST", "/books"), Some(BookRoute::CreateBook));

    // A captured variable is read straight off the matched variant's field.
    let Some(BookRoute::GetBook { book }) = BookResolver.resolve("GET", "/books/rust-in-action") else {
        unreachable!("expected GetBook");
    };
    assert_eq!(book, "rust-in-action");

    // Several captures are each their own field.
    let Some(BookRoute::GetReview { book, review }) = BookResolver.resolve("GET", "/books/rust-in-action/reviews/42") else {
        unreachable!("expected GetReview");
    };
    assert_eq!((book, review), ("rust-in-action", "42"));

    // A custom `:verb` selects a distinct route on the same method + path shape.
    assert!(matches!(
        BookResolver.resolve("POST", "/books/rust-in-action:archive"),
        Some(BookRoute::ArchiveBook { book }) if book == "rust-in-action"
    ));

    // The extended grammar wraps a capture in literal text within one segment —
    // here a `cover-{size}.png` prefix/suffix. `#[resolver]` accepts it directly.
    assert!(matches!(
        BookResolver.resolve("GET", "/books/rust-in-action/cover-large.png"),
        Some(BookRoute::GetCover { book, size }) if book == "rust-in-action" && size == "large"
    ));

    // Misses (wrong method, unknown path) resolve to `None`.
    assert!(BookResolver.resolve("DELETE", "/books").is_none());
    assert!(BookResolver.resolve("GET", "/authors/tolkien").is_none());

    // Dispatching is an `O(1)` `match` over the `Copy` enum — no string
    // comparison, captures read straight from the variant.
    for (method, path) in [
        ("GET", "/books"),
        ("GET", "/books/rust-in-action"),
        ("GET", "/books/rust-in-action/reviews/42"),
        ("POST", "/books/rust-in-action:archive"),
        ("GET", "/books/rust-in-action/cover-large.png"),
        ("GET", "/search"),
        ("DELETE", "/books"),
    ] {
        let action = match BookResolver.resolve(method, path) {
            Some(BookRoute::ListBooks) => "list books".to_owned(),
            Some(BookRoute::CreateBook) => "create a book".to_owned(),
            Some(BookRoute::GetBook { book }) => format!("get book {book}"),
            Some(BookRoute::GetReview { book, review }) => format!("get review {review} of book {book}"),
            Some(BookRoute::ArchiveBook { book }) => format!("archive book {book}"),
            Some(BookRoute::GetCover { book, size }) => format!("get {size} cover of book {book}"),
            Some(BookRoute::SearchBooks) => "search books".to_owned(),
            None => "404".to_owned(),
        };
        println!("{method:<6} {path:<40} -> {action}");
    }
}
