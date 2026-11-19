// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Tests of the route table of `#[resolver]` types.

#![allow(clippy::unwrap_used, reason = "test code")]
#![expect(
    dead_code,
    reason = "most tests read the templates and raw values of the route enums, not their captured fields"
)]

use http_path_template::{Grammar, PathTemplate};
use routerama::__rt::{RawResolver, Route};
use routerama::{HttpMethod, ResolveError, RouteTable, resolver};

/// Matches `GET path`, and returns the template of the route and the value of the parameter `name`. It takes a
/// `dyn RouteTable`, as a telemetry layer keeps one.
fn hit<'m, 'p>(table: &'m dyn RouteTable, path: &'p str, name: &str) -> Option<(&'m str, Option<&'p str>)> {
    let hit = table.match_route("GET", path)?;
    Some((hit.template().as_ref(), hit.parameter(name)))
}

#[resolver]
enum ProfileRoute<'p> {
    #[route(GET, "/status")]
    Status,
    #[route(GET, "/users/{user}")]
    #[route(GET, "/accounts/{user}")]
    User { user: &'p str },
}

#[test]
fn static_resolver_matches_each_route_attribute() {
    let resolver = ProfileRoute::resolver();

    assert_eq!(
        ["/status", "/users/7", "/accounts/7"].map(|path| hit(&resolver, path, "user")),
        [
            Some(("/status", None)),
            Some(("/users/{user}", Some("7"))),
            Some(("/accounts/{user}", Some("7"))),
        ]
    );
}

#[resolver]
enum PluginRoute<'p> {
    #[route(GET, "/books/{book}")]
    GetBook {
        book: &'p str,
    },
    Plugin {
        name: String,
    },
}

#[test]
fn hybrid_resolver_matches_static_and_dynamic_routes() {
    let resolver = PluginRoute::builder()
        .add_plugin(HttpMethod::GET, "/plugins/{name}")
        .build()
        .unwrap();

    assert_eq!(
        [("/books/rust", "book"), ("/plugins/auth", "name")].map(|(path, name)| hit(&resolver, path, name)),
        [Some(("/books/{book}", Some("rust"))), Some(("/plugins/{name}", Some("auth")))]
    );
}

#[resolver]
enum ExoticRoute<'p> {
    #[route(GET, "/x/{type}")]
    Typed { _f_type: &'p str },
    #[route(GET, "/shelves/{shelf.id}")]
    Dotted { shelf_id: &'p str },
}

#[test]
fn dotted_and_keyword_captures_keep_template_names() {
    let resolver = ExoticRoute::resolver();

    assert_eq!(
        [("/shelves/7", "shelf.id"), ("/x/1", "type")].map(|(path, name)| hit(&resolver, path, name)),
        [Some(("/shelves/{shelf.id}", Some("7"))), Some(("/x/{type}", Some("1")))]
    );
}

#[resolver]
enum TemplateRoute<'p> {
    #[route(GET, "/books")]
    ListBooks,
    #[route(POST, "/books/{book}:archive")]
    Archive { book: &'p str },
    #[route(GET, "/files/{path=**}")]
    Files { path: &'p str },
    #[route(GET, "/static/**")]
    Static,
    #[route(GET, "/img-{id}.png")]
    Image { id: &'p str },
    #[route(GET, "/shelves/{shelf.id}/books/{book}")]
    ShelfBook { shelf_id: &'p str, book: &'p str },
}

#[test]
fn generated_and_raw_resolvers_match_same_routes() {
    let raw = RawResolver::new(
        [
            ("ListBooks", HttpMethod::GET, "/books"),
            ("Archive", HttpMethod::POST, "/books/{book}:archive"),
            ("Files", HttpMethod::GET, "/files/{path=**}"),
            ("Static", HttpMethod::GET, "/static/**"),
            ("Image", HttpMethod::GET, "/img-{id}.png"),
            ("ShelfBook", HttpMethod::GET, "/shelves/{shelf.id}/books/{book}"),
        ]
        .map(|(name, method, template)| {
            Route::new(
                name,
                method,
                PathTemplate::parse(template, Grammar::default().with_segment_affixes()).unwrap(),
            )
        }),
    );

    let matches = |table: &dyn RouteTable| {
        [
            ("GET", "/books"),
            ("POST", "/books/rust:archive"),
            ("GET", "/files/a/b"),
            ("GET", "/static/a/b"),
            ("GET", "/img-cat.png"),
            ("GET", "/shelves/7/books/rust"),
        ]
        .map(|(method, path)| {
            table.match_route(method, path).map(|hit| {
                let parameters = ["book", "path", "id", "shelf.id"].map(|name| hit.parameter(name));
                (hit.template().to_string(), parameters)
            })
        })
    };
    assert_eq!(matches(&TemplateRoute::resolver()), matches(&raw));
}

#[resolver]
enum ApiRoute<'p> {
    #[route(GET, "/books")]
    ListBooks,
    #[route(POST, "/books")]
    CreateBook,
    #[route(GET, "/books/featured")]
    GetFeatured,
    #[route(GET, "/books/{book}")]
    GetBook { book: &'p str },
    #[route(GET, "/books/{book}/reviews/{review}")]
    GetReview { book: &'p str, review: &'p str },
    #[route(POST, "/books/{book}:archive")]
    Archive { book: &'p str },
    #[route(GET, "/files/**")]
    Files,
    #[route(GET, "/search")]
    Search,
}

#[test]
fn match_route_agrees_with_resolve_on_parity_corpus() {
    let resolver = ApiRoute::resolver();
    let requests = [
        ("GET", "/books"),
        ("POST", "/books"),
        ("GET", "/books/featured"),
        ("GET", "/books/rust"),
        ("GET", "/books/rust/reviews/42"),
        ("POST", "/books/rust:archive"),
        ("POST", "/books/rust"),
        ("GET", "/files/a/b/c"),
        ("GET", "/files"),
        ("GET", "/search"),
        ("DELETE", "/books"),
        ("GET", "/unknown"),
        ("GET", "/books/rust/extra"),
        ("GET", "/books/featured/x"),
    ];

    for (method, path) in requests {
        let from_resolve = resolver.resolve(method, path).ok().map(|route| match route {
            ApiRoute::ListBooks | ApiRoute::CreateBook => ("/books", vec![]),
            ApiRoute::GetFeatured => ("/books/featured", vec![]),
            ApiRoute::GetBook { book } => ("/books/{book}", vec![book]),
            ApiRoute::GetReview { book, review } => ("/books/{book}/reviews/{review}", vec![book, review]),
            ApiRoute::Archive { book } => ("/books/{book}:archive", vec![book]),
            ApiRoute::Files => ("/files/**", vec![]),
            ApiRoute::Search => ("/search", vec![]),
        });
        let from_match_route = resolver.match_route(method, path).map(|hit| {
            (
                hit.template().as_ref(),
                ["book", "review"].into_iter().filter_map(|name| hit.parameter(name)).collect(),
            )
        });
        assert_eq!(from_resolve, from_match_route, "`{method} {path}`");
    }
}

#[resolver]
enum DynamicVerbRoute<'p> {
    #[route(GET, "/items/{id}")]
    GetItem {
        id: &'p str,
    },
    Archive {
        id: String,
    },
}

#[resolver]
enum StaticVerbRoute<'p> {
    #[route(GET, "/docs/{id}:publish")]
    Publish {
        id: &'p str,
    },
    Get {
        id: String,
    },
}

#[test]
fn verb_route_prefers_dynamic_like_resolve() {
    let dynamic_verb = DynamicVerbRoute::builder()
        .add_archive(HttpMethod::GET, "/items/{id}:archive")
        .build()
        .unwrap();
    let static_verb = StaticVerbRoute::builder().add_get(HttpMethod::GET, "/docs/{id}").build().unwrap();

    let dynamic_verb_paths = ["/items/7:archive", "/items/7", "/items/7:nope"];
    let from_resolve: Vec<Option<&str>> = dynamic_verb_paths
        .iter()
        .map(|path| match dynamic_verb.resolve("GET", path) {
            Ok(DynamicVerbRoute::GetItem { .. }) => Some("/items/{id}"),
            Ok(DynamicVerbRoute::Archive { .. }) => Some("/items/{id}:archive"),
            Err(_) => None,
        })
        .collect();
    let from_match_route: Vec<Option<&str>> = dynamic_verb_paths
        .iter()
        .map(|path| dynamic_verb.match_route("GET", path).map(|hit| hit.template().as_ref()))
        .collect();
    assert_eq!(from_resolve, [Some("/items/{id}:archive"), Some("/items/{id}"), None]);
    assert_eq!(from_match_route, from_resolve);

    let static_verb_paths = ["/docs/7:publish", "/docs/7", "/docs/7:other"];
    let from_resolve: Vec<Option<&str>> = static_verb_paths
        .iter()
        .map(|path| match static_verb.resolve("GET", path) {
            Ok(StaticVerbRoute::Publish { .. }) => Some("/docs/{id}:publish"),
            Ok(StaticVerbRoute::Get { .. }) => Some("/docs/{id}"),
            Err(_) => None,
        })
        .collect();
    let from_match_route: Vec<Option<&str>> = static_verb_paths
        .iter()
        .map(|path| static_verb.match_route("GET", path).map(|hit| hit.template().as_ref()))
        .collect();
    assert_eq!(from_resolve, [Some("/docs/{id}:publish"), Some("/docs/{id}"), None]);
    assert_eq!(from_match_route, from_resolve);
}

#[resolver]
#[derive(Debug, PartialEq, Eq)]
enum IssueRoute {
    #[route(GET, "/issues/{issue}")]
    Issue {
        issue: u32,
    },
    Named {
        name: String,
    },
}

#[test]
fn match_route_reports_route_when_typed_capture_fails() {
    let resolver = IssueRoute::builder().add_named(HttpMethod::GET, "/issues/{name}").build().unwrap();

    let hit = resolver.match_route("GET", "/issues/abc").unwrap();

    assert_eq!(resolver.resolve("GET", "/issues/abc"), Err(ResolveError::InvalidCapture("issue")));
    assert_eq!((hit.template().as_ref(), hit.parameter("issue")), ("/issues/{issue}", Some("abc")));
}
