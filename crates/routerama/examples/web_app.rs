// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! A tiny HTTP server where `routerama` does all the routing
//! with [`axum`] used only for the transport.
//!
//! Instead of registering routes with the router from `axum`, every request falls
//! through to a single [`fallback`](axum::Router::fallback) handler that hands
//! the method and path to a [`#[resolver]`](routerama::resolver) and matches on
//! the typed result. The list route separately decodes and produces a typed
//! query string.

use std::sync::Arc;

use axum::Router;
use axum::extract::State;
use axum::http::{Method, StatusCode, Uri};
use routerama::query::{FromQuery, ToQuery};
use routerama::{ResolveError, resolver};
use tokio::net::TcpListener;

#[derive(Clone, Debug, FromQuery, ToQuery)]
struct BooksQuery {
    q: Option<String>,
    page: Option<usize>,
    tag: Vec<String>,
}

/// The application's routing table. Captures are typed and validated against the
/// path template at compile time.
#[resolver]
enum Route<'p> {
    #[route(GET, "/books")]
    ListBooks,

    #[route(GET, "/books/{id}")]
    GetBook { id: u32 },

    #[route(GET, "/hello/{name}")]
    Greet { name: String },

    #[route(GET, "/echo/{word}")]
    Echo { word: &'p str },
}

/// Shared, read-only application state.
struct AppState {
    resolver: RouteResolver,
    books: Vec<(u32, &'static str)>,
}

/// The single handler: axum extracts the method and URI, `routerama`
/// does all the routing, and each typed `Route` becomes a response.
async fn dispatch(State(state): State<Arc<AppState>>, method: Method, uri: Uri) -> (StatusCode, String) {
    match state.resolver.resolve(method.as_str(), uri.path()) {
        Ok(Route::ListBooks) => list_books(&state.books, uri.query().unwrap_or_default()),
        Ok(Route::GetBook { id }) => match state.books.iter().find(|(book_id, _)| *book_id == id) {
            Some((id, title)) => (StatusCode::OK, format!("{id}: {title}\n")),
            None => (StatusCode::NOT_FOUND, format!("no book with id {id}\n")),
        },
        Ok(Route::Greet { name }) => (StatusCode::OK, format!("Hello, {name}!\n")),
        Ok(Route::Echo { word }) => (StatusCode::OK, format!("{word}\n")),
        Err(error @ (ResolveError::MissingCapture(_) | ResolveError::InvalidCapture(_) | ResolveError::UndecodableCapture(_))) => {
            (StatusCode::BAD_REQUEST, format!("bad request: {error}\n"))
        }
        Err(ResolveError::NotFound(_)) => (StatusCode::NOT_FOUND, "nothing here\n".to_owned()),
        Err(error) => (StatusCode::INTERNAL_SERVER_ERROR, format!("routing error: {error}\n")),
    }
}

fn list_books(books: &[(u32, &'static str)], query: &str) -> (StatusCode, String) {
    use std::fmt::Write as _;

    let Ok(query) = BooksQuery::from_query(query) else {
        return (StatusCode::BAD_REQUEST, "invalid query string\n".to_owned());
    };

    let page = query.page.unwrap_or(1);
    let Some(offset) = page.checked_sub(1).and_then(|page| page.checked_mul(2)) else {
        return (StatusCode::BAD_REQUEST, "page is out of range\n".to_owned());
    };
    let Some(next_page) = page.checked_add(1) else {
        return (StatusCode::BAD_REQUEST, "page is out of range\n".to_owned());
    };
    let search = query.q.as_deref().map(str::to_ascii_lowercase);
    let mut body = String::new();
    for (id, title) in books
        .iter()
        .filter(|(_, title)| search.as_ref().is_none_or(|q| title.to_ascii_lowercase().contains(q)))
        .skip(offset)
        .take(2)
    {
        let _ = writeln!(body, "{id}: {title}");
    }

    let next = BooksQuery {
        page: Some(next_page),
        ..query
    };
    if let Ok(next) = next.to_query_string() {
        let _ = writeln!(body, "next: /books?{next}");
    }
    (StatusCode::OK, body)
}

#[tokio::main]
async fn main() {
    let resolver = Route::resolver();

    let state = Arc::new(AppState {
        resolver,
        books: vec![
            (1, "The Rust Programming Language"),
            (2, "Rust for Rustaceans"),
            (3, "Programming Rust"),
        ],
    });

    let listener = TcpListener::bind("127.0.0.1:8080").await.expect("failed to bind 127.0.0.1:8080");

    let app = Router::new().fallback(dispatch).with_state(state);
    let server = axum::serve(listener, app);

    if std::env::var_os("IS_TESTING").is_some() {
        server.with_graceful_shutdown(async {}).await.expect("server error");
    } else {
        server.await.expect("server error");
    }
}
