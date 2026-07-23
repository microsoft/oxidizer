// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Routerama routing behind an Axum transport fallback.

use std::sync::Arc;

use axum::Router;
use axum::extract::State as AxumState;
use axum::http::StatusCode;
use routerama::query::{FromQuery, ToQuery};
use routerama::response::Response;
use routerama::route::{Query, Request, router};
use tokio::net::TcpListener;

#[derive(Clone, Debug, FromQuery, ToQuery)]
struct BooksQuery {
    q: Option<String>,
    page: Option<usize>,
    tag: Vec<String>,
}

/// Shared, read-only application state.
struct AppState {
    books: Vec<(u32, &'static str)>,
}

#[allow(
    clippy::allow_attributes,
    unknown_lints,
    clippy::unused_async,
    clippy::unused_async_trait_impl,
    reason = "router handlers must be async; `clippy::unused_async_trait_impl` does not exist before clippy 0.1.98"
)]
#[router]
impl AppState {
    #[route(GET, "/books")]
    async fn list_books(&self, query: Query<BooksQuery>) -> (StatusCode, String) {
        list_books(&self.books, query.into_inner())
    }

    #[route(GET, "/books/{id}")]
    async fn get_book(&self, id: u32) -> (StatusCode, String) {
        match self.books.iter().find(|(book_id, _)| *book_id == id) {
            Some((id, title)) => (StatusCode::OK, format!("{id}: {title}\n")),
            None => (StatusCode::NOT_FOUND, format!("no book with id {id}\n")),
        }
    }

    #[route(GET, "/hello/{name}")]
    async fn greet(&self, name: String) -> (StatusCode, String) {
        (StatusCode::OK, format!("Hello, {name}!\n"))
    }

    #[route(GET, "/echo/{word}")]
    async fn echo(&self, word: &str) -> (StatusCode, String) {
        (StatusCode::OK, format!("{word}\n"))
    }
}

/// The single handler hands request ownership to Routerama, which dispatches
/// directly to the matching `AppState` method. Axum's transport boundary adds
/// its `Send + 'static` body and error requirements here rather than in core
/// routing.
async fn dispatch(AxumState(state): AxumState<Arc<AppState>>, request: Request<axum::body::Body>) -> Response<axum::body::Body> {
    state.route(request, &()).await.map(axum::body::Body::new)
}

fn list_books(books: &[(u32, &'static str)], query: BooksQuery) -> (StatusCode, String) {
    use std::fmt::Write as _;

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
    let state = Arc::new(AppState {
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
