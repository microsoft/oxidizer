// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Tests of the route table of `#[resolver]` types.

#![allow(clippy::unwrap_used, reason = "test code")]
#![allow(
    dead_code,
    reason = "most tests read the route tables of the route enums, not their captured fields"
)]

use std::fmt::Write as _;

use http_path_template::{Grammar, PathTemplate};
use routerama::__rt::{RawResolver, Route};
use routerama::{HttpMethod, ResolveError, RouteTable, resolver};

/// Reads the template and the parameter names of each route, sorted, because the numbers of the routes follow no
/// order. It takes a `dyn RouteTable`, as a telemetry layer keeps one.
fn listing(table: &dyn RouteTable) -> Vec<(&str, Vec<&str>)> {
    let mut routes: Vec<(&str, Vec<&str>)> = (0..table.route_count())
        .map(|route| {
            let parameters = table.parameters(route).unwrap();
            (table.template(route).unwrap(), parameters.iter().map(AsRef::as_ref).collect())
        })
        .collect();
    routes.sort_unstable();
    routes
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
#[cfg_attr(
    miri,
    ignore = "insta reads snapshot files; the other listing tests in this file check the same data under Miri"
)]
fn static_resolver_lists_each_route_attribute() {
    let resolver = ProfileRoute::resolver();

    let mut printed = String::new();
    for (template, parameters) in listing(&resolver) {
        writeln!(printed, "{template} [{}]", parameters.join(", ")).unwrap();
    }

    insta::assert_snapshot!(printed);
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
fn hybrid_resolver_lists_static_and_dynamic_routes() {
    let resolver = PluginRoute::builder()
        .add_plugin(HttpMethod::GET, "/plugins/{name}")
        .build()
        .unwrap();

    assert_eq!(
        listing(&resolver),
        [("/books/{book}", vec!["book"]), ("/plugins/{name}", vec!["name"])]
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
        listing(&resolver),
        [("/shelves/{shelf.id}", vec!["shelf.id"]), ("/x/{type}", vec!["type"])]
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
fn generated_and_raw_tables_print_same_templates() {
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

    assert_eq!(listing(&TemplateRoute::resolver()), listing(&raw));
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
        let from_match_route = resolver
            .match_route(method, path)
            .map(|hit| (resolver.template(hit.route()).unwrap(), hit.captures().to_vec()));
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
        .map(|path| {
            dynamic_verb
                .match_route("GET", path)
                .map(|hit| dynamic_verb.template(hit.route()).unwrap())
        })
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
        .map(|path| {
            static_verb
                .match_route("GET", path)
                .map(|hit| static_verb.template(hit.route()).unwrap())
        })
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
    assert_eq!(
        (resolver.template(hit.route()), hit.captures()),
        (Some("/issues/{issue}"), ["abc"].as_slice())
    );
}
