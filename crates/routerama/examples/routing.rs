// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Static typed resolution with borrowed, decoded, and parsed captures.

use std::borrow::Cow;
use std::str::FromStr;

use routerama::resolve::ResolveError;

/// Custom capture type parsed through `FromStr`.
#[derive(Debug, PartialEq, Eq)]
struct Isbn(u64);

impl FromStr for Isbn {
    type Err = std::num::ParseIntError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        s.replace('-', "").parse().map(Isbn)
    }
}

#[routerama::resolve::resolver]
#[derive(Debug, PartialEq, Eq)]
enum BookRoute<'p> {
    #[route(GET, "/books")]
    ListBooks,

    #[route(POST, "/books")]
    CreateBook,

    #[route(GET, "/books/{book}")]
    GetBook { book: &'p str },

    #[route(GET, "/books/{book}/reviews/{review}")]
    GetReview { book: &'p str, review: u32 },

    #[route(POST, "/books/{book}:archive")]
    ArchiveBook { book: &'p str },

    #[route(GET, "/books/{book}/cover-{size}.png")]
    GetCover { book: &'p str, size: &'p str },

    #[route(GET, "/authors/{name}")]
    GetAuthor { name: String },

    #[route(GET, "/files/{path=**}")]
    GetFile { path: Cow<'p, str> },

    #[route(GET, "/isbn/{code}")]
    GetByIsbn { code: Isbn },

    #[route(GET, "/search")]
    SearchBooks,
}

fn main() {
    let resolver = BookRoute::resolver();

    let Ok(BookRoute::GetReview { book, review }) = resolver.resolve("GET", "/books/rust-in-action/reviews/42") else {
        unreachable!("expected GetReview");
    };
    assert_eq!((book, review), ("rust-in-action", 42_u32));

    assert!(matches!(
        resolver.resolve("POST", "/books/rust-in-action:archive"),
        Ok(BookRoute::ArchiveBook { book }) if book == "rust-in-action"
    ));

    assert!(matches!(
        resolver.resolve("GET", "/books/rust-in-action/cover-large.png"),
        Ok(BookRoute::GetCover { book, size }) if book == "rust-in-action" && size == "large"
    ));

    let Ok(BookRoute::GetAuthor { name }) = resolver.resolve("GET", "/authors/Ursula%20K.%20Le%20Guin") else {
        unreachable!("expected GetAuthor");
    };
    assert_eq!(name, "Ursula K. Le Guin");

    assert_eq!(
        resolver.resolve("GET", "/isbn/978-0-13-468599-1"),
        Ok(BookRoute::GetByIsbn {
            code: Isbn(9_780_134_685_991)
        })
    );

    assert_eq!(
        resolver.resolve("GET", "/books/rust/reviews/not-a-number"),
        Err(ResolveError::InvalidCapture("review"))
    );

    assert_eq!(resolver.resolve("DELETE", "/books"), Err(ResolveError::NotFound("/books")));
    assert_eq!(resolver.resolve("GET", "/nope/nope"), Err(ResolveError::NotFound("/nope/nope")));

    for (method, path) in [
        ("GET", "/books"),
        ("GET", "/books/rust-in-action"),
        ("GET", "/books/rust-in-action/reviews/42"),
        ("POST", "/books/rust-in-action:archive"),
        ("GET", "/books/rust-in-action/cover-large.png"),
        ("GET", "/authors/Ursula%20K.%20Le%20Guin"),
        ("GET", "/files/manuals/rust.pdf"),
        ("GET", "/isbn/978-0-13-468599-1"),
        ("GET", "/search"),
        ("DELETE", "/books"),
    ] {
        let action = match resolver.resolve(method, path) {
            Ok(BookRoute::ListBooks) => "list books".to_owned(),
            Ok(BookRoute::CreateBook) => "create a book".to_owned(),
            Ok(BookRoute::GetBook { book }) => format!("get book {book}"),
            Ok(BookRoute::GetReview { book, review }) => format!("get review {review} of book {book}"),
            Ok(BookRoute::ArchiveBook { book }) => format!("archive book {book}"),
            Ok(BookRoute::GetCover { book, size }) => format!("get {size} cover of book {book}"),
            Ok(BookRoute::GetAuthor { name }) => format!("get author {name}"),
            Ok(BookRoute::GetFile { path }) => format!("get file {path}"),
            Ok(BookRoute::GetByIsbn { code }) => format!("get book by isbn {}", code.0),
            Ok(BookRoute::SearchBooks) => "search books".to_owned(),
            Err(error @ (ResolveError::MissingCapture(_) | ResolveError::InvalidCapture(_) | ResolveError::UndecodableCapture(_))) => {
                format!("400: {error}")
            }
            Err(ResolveError::NotFound(_)) => "404".to_owned(),
            Err(error) => format!("routing error: {error}"),
        };
        println!("{method:<6} {path:<40} -> {action}");
    }
}
