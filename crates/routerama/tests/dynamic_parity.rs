// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Differential tests for static and runtime resolution.

use http_path_template::{Grammar, PathTemplate};
use routerama::__rt::{RawResolver, Route, RouteMatch};
use routerama::{HttpMethod, ResolveError};

#[routerama::resolver]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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

/// Returns the route name used by the runtime resolver.
fn static_name(route: ApiRoute<'_>) -> &'static str {
    match route {
        ApiRoute::ListBooks => "ListBooks",
        ApiRoute::CreateBook => "CreateBook",
        ApiRoute::GetFeatured => "GetFeatured",
        ApiRoute::GetBook { .. } => "GetBook",
        ApiRoute::GetReview { .. } => "GetReview",
        ApiRoute::Archive { .. } => "Archive",
        ApiRoute::Files => "Files",
        ApiRoute::Search => "Search",
    }
}

/// Returns a named static capture for backend comparison.
fn static_capture<'p>(route: ApiRoute<'p>, field: &str) -> Option<&'p str> {
    match route {
        ApiRoute::GetBook { book } | ApiRoute::Archive { book } => (field == "book").then_some(book),
        ApiRoute::GetReview { book, review } => match field {
            "book" => Some(book),
            "review" => Some(review),
            _ => None,
        },
        ApiRoute::Files | ApiRoute::ListBooks | ApiRoute::CreateBook | ApiRoute::GetFeatured | ApiRoute::Search => None,
    }
}

/// Resolves a route using the static backend.
fn static_resolve<'p>(method: &str, path: &'p str) -> Option<ApiRoute<'p>> {
    let resolver = ApiRoute::resolver();
    match resolver.resolve(method, path) {
        Err(ResolveError::InvalidPath(_) | ResolveError::NotFound(_)) => None,
        Err(ResolveError::MissingCapture(_) | ResolveError::InvalidCapture(_) | ResolveError::UndecodableCapture(_)) => {
            unreachable!("ApiRoute has only `&str` captures, which never fail to coerce")
        }
        Err(_) => unreachable!("unknown resolution error"),
        Ok(route) => Some(route),
    }
}

fn dynamic_resolver() -> RawResolver {
    RawResolver::new([
        Route::new(
            "ListBooks",
            HttpMethod::GET,
            PathTemplate::parse("/books", Grammar::default()).expect("valid"),
        ),
        Route::new(
            "CreateBook",
            HttpMethod::POST,
            PathTemplate::parse("/books", Grammar::default()).expect("valid"),
        ),
        Route::new(
            "GetFeatured",
            HttpMethod::GET,
            PathTemplate::parse("/books/featured", Grammar::default()).expect("valid"),
        ),
        Route::new(
            "GetBook",
            HttpMethod::GET,
            PathTemplate::parse("/books/{book}", Grammar::default()).expect("valid"),
        ),
        Route::new(
            "GetReview",
            HttpMethod::GET,
            PathTemplate::parse("/books/{book}/reviews/{review}", Grammar::default()).expect("valid"),
        ),
        Route::new(
            "Archive",
            HttpMethod::POST,
            PathTemplate::parse("/books/{book}:archive", Grammar::default()).expect("valid"),
        ),
        Route::new(
            "Files",
            HttpMethod::GET,
            PathTemplate::parse("/files/**", Grammar::default()).expect("valid"),
        ),
        Route::new(
            "Search",
            HttpMethod::GET,
            PathTemplate::parse("/search", Grammar::default()).expect("valid"),
        ),
    ])
}

#[routerama::resolver]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ExoticRoute<'p> {
    #[route(GET, "/x/{type}")]
    Typed { _f_type: &'p str },
    #[route(GET, "/shelves/{shelf.id}")]
    Dotted { shelf_id: &'p str },
}

fn exotic_dynamic_resolver() -> RawResolver {
    RawResolver::new([
        Route::new(
            "Typed",
            HttpMethod::GET,
            PathTemplate::parse("/x/{type}", Grammar::default()).expect("valid"),
        ),
        Route::new(
            "Dotted",
            HttpMethod::GET,
            PathTemplate::parse("/shelves/{shelf.id}", Grammar::default()).expect("valid"),
        ),
    ])
}

/// Exotic capture names use sanitized fields and original lookup keys.
#[test]
fn exotic_capture_names_bind_on_both_backends() {
    match exotic_resolver().resolve("GET", "/x/int") {
        Ok(ExoticRoute::Typed { _f_type: got }) => assert_eq!(got, "int"),
        other => panic!("expected Typed, got {other:?}"),
    }
    match exotic_resolver().resolve("GET", "/shelves/sci-fi") {
        Ok(ExoticRoute::Dotted { shelf_id }) => assert_eq!(shelf_id, "sci-fi"),
        other => panic!("expected Dotted, got {other:?}"),
    }

    let resolver = exotic_dynamic_resolver();
    let typed = resolver.resolve("GET", "/x/int").expect("match");
    assert_eq!(typed.capture("type"), Some("int"));
    assert_eq!(typed.capture("_f_type"), None); // not the sanitized identifier
    let dotted = resolver.resolve("GET", "/shelves/sci-fi").expect("match");
    assert_eq!(dotted.capture("shelf.id"), Some("sci-fi"));
    assert_eq!(dotted.capture("shelf_id"), None);
}

/// Static and runtime backends select the same route.
#[test]
fn dynamic_agrees_with_static_on_every_request() {
    let resolver = dynamic_resolver();
    let cases = [
        ("GET", "/books"),
        ("POST", "/books"),
        ("GET", "/books/featured"), // literal beats the `{book}` wildcard
        ("GET", "/books/rust"),
        ("GET", "/books/rust/reviews/42"),
        ("POST", "/books/rust:archive"),
        ("POST", "/books/rust"), // the verb is required: no bare-POST route here
        ("GET", "/files/a/b/c"), // `**` catch-all
        ("GET", "/files"),       // `**` matches an empty remainder
        ("GET", "/search"),
        ("DELETE", "/books"),         // unknown method
        ("GET", "/unknown"),          // unknown path
        ("GET", "/books/rust/extra"), // too deep for GetBook, no route
        ("GET", "/books/featured/x"), // deeper than the literal
    ];
    for (method, path) in cases {
        let oracle = static_resolve(method, path).map(|route| static_name(route).to_owned());
        let dynamic = resolver.resolve(method, path).map(|matched| matched.name().to_owned());
        assert_eq!(oracle, dynamic, "static/dynamic disagree on `{method} {path}`");
    }
}

/// Static and runtime backends capture the same values.
#[test]
fn dynamic_captures_match_the_static_router() {
    let resolver = dynamic_resolver();

    let review = resolver.resolve("GET", "/books/rust/reviews/42").expect("match");
    assert_eq!(review.name(), "GetReview");
    assert_eq!(review.capture("book"), Some("rust"));
    assert_eq!(review.capture("review"), Some("42"));
    assert_eq!(review.capture("missing"), None);

    match static_resolve("GET", "/books/rust/reviews/42").expect("match") {
        ApiRoute::GetReview { book, review } => {
            assert_eq!(book, "rust");
            assert_eq!(review, "42");
        }
        other => panic!("static resolver matched the wrong route: {other:?}"),
    }

    let featured = resolver.resolve("GET", "/books/featured").expect("match");
    assert_eq!(featured.name(), "GetFeatured");
    assert_eq!(featured.captures().count(), 0);
}

/// Both resolver backends accept an [`HttpMethod`] through `AsRef<str>`.
#[test]
fn resolve_accepts_a_typed_http_method() {
    assert!(matches!(
        ApiRoute::resolver().resolve(HttpMethod::POST, "/books"),
        Ok(ApiRoute::CreateBook)
    ));
    assert_eq!(static_resolve("GET", "/books").map(static_name), Some("ListBooks"));

    let resolver = dynamic_resolver();
    let dynamic = resolver.resolve(HttpMethod::GET, "/books/rust").expect("match");
    assert_eq!(dynamic.name(), "GetBook");
    assert_eq!(dynamic.capture("book"), Some("rust"));

    assert!(
        resolver
            .resolve(HttpMethod::custom("GET").expect("GET is a valid method token"), "/search")
            .is_some()
    );
    assert!(resolver.resolve(HttpMethod::DELETE, "/books").is_none());
}

/// `resolve` accepts any `AsRef<str>` path (a `String`, `Box<str>`, …), not only
/// `&str`, on both backends.
#[test]
fn resolve_accepts_a_non_str_path() {
    let resolver = dynamic_resolver();
    let owned: String = "/books/rust".to_owned();
    let matched = resolver.resolve("GET", &owned).expect("match");
    assert_eq!(matched.name(), "GetBook");
    assert_eq!(matched.capture("book"), Some("rust"));

    let boxed: Box<str> = "/books".into();
    assert_eq!(static_resolve("GET", &boxed).map(static_name), Some("ListBooks"));
}

/// A static resolver can fall back to a runtime overlay.
#[test]
fn static_core_falls_back_to_a_dynamic_overlay() {
    let plugins = RawResolver::new([Route::new(
        "Plugin",
        HttpMethod::GET,
        PathTemplate::parse("/plugins/{name}", Grammar::default()).expect("valid"),
    )]);

    let dispatch = |method: &str, path: &str| -> Option<String> {
        if let Some(route) = static_resolve(method, path) {
            Some(static_name(route).to_owned())
        } else {
            plugins.resolve(method, path).map(|plugin| {
                assert_eq!(plugin.capture("name"), Some("auth"));
                plugin.name().to_owned()
            })
        }
    };

    assert_eq!(dispatch("GET", "/books/rust").as_deref(), Some("GetBook"));
    assert_eq!(dispatch("GET", "/plugins/auth").as_deref(), Some("Plugin"));
    assert_eq!(dispatch("GET", "/nope"), None);
}

// Covers lifetime-free generated enums and static promotion of their resolver.
#[routerama::resolver]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Ping {
    #[route(GET, "/health")]
    Health,
    #[route(GET, "/ready")]
    Ready,
}

#[test]
fn capture_less_resolver_resolves_its_routes() {
    assert_eq!(ping_resolver().resolve("GET", "/health"), Ok(Ping::Health));
    assert_eq!(ping_resolver().resolve("GET", "/ready"), Ok(Ping::Ready));
    assert!(matches!(ping_resolver().resolve("POST", "/health"), Err(ResolveError::NotFound(_))));
    assert!(matches!(ping_resolver().resolve("GET", "/nope"), Err(ResolveError::NotFound(_))));
}

/// Compares both backends over a generated request space.
#[test]
fn dynamic_agrees_with_static_over_a_generated_path_space() {
    let resolver = dynamic_resolver();
    let segments = ["books", "featured", "rust", "reviews", "42", "search", "files", "x", ""];
    let methods = ["GET", "POST", "DELETE"];
    let verbs = ["", ":archive", ":other"];

    let mut checked = 0_u64;
    for method in methods {
        for depth in 0..=4 {
            let mut indices = vec![0_usize; depth];
            loop {
                let mut path = String::new();
                for &i in &indices {
                    path.push('/');
                    path.push_str(segments[i]);
                }
                for verb in verbs {
                    let full = if path.is_empty() {
                        format!("/{verb}")
                    } else {
                        format!("{path}{verb}")
                    };

                    let oracle = static_resolve(method, &full);
                    let dynamic = resolver.resolve(method, &full);

                    let oracle_name = oracle.map(static_name);
                    let dynamic_name = dynamic.as_ref().map(RouteMatch::name);
                    assert_eq!(oracle_name, dynamic_name, "name disagreement on `{method} {full}`");

                    if let (Some(route), Some(matched)) = (oracle, dynamic) {
                        for field in ["book", "review"] {
                            let from_static = static_capture(route, field);
                            let from_dynamic = matched.capture(field);
                            assert_eq!(from_static, from_dynamic, "capture `{field}` disagreement on `{method} {full}`");
                        }
                    }
                    checked += 1;
                }

                if depth == 0 {
                    break;
                }
                let mut pos = depth;
                loop {
                    if pos == 0 {
                        break;
                    }
                    pos -= 1;
                    indices[pos] += 1;
                    if indices[pos] < segments.len() {
                        break;
                    }
                    indices[pos] = 0;
                    if pos == 0 {
                        pos = usize::MAX;
                        break;
                    }
                }
                if pos == usize::MAX {
                    break;
                }
            }
        }
    }
    assert!(checked > 50_000, "expected a large path space, checked {checked}");
}

fn exotic_resolver() -> ExoticRouteResolver {
    ExoticRoute::resolver()
}
fn ping_resolver() -> PingResolver {
    Ping::resolver()
}
