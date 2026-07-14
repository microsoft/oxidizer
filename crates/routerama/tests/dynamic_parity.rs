// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Differential tests: the runtime [`DynResolver`] must resolve a route set
//! *identically* to the static `#[resolver]` resolver, since both walk the
//! same shared trie. The static router is the oracle.

#![cfg(all(feature = "dynamic", feature = "macros"))]

use http_path_template::{Grammar, PathTemplate};
use routerama::{DynResolver, HttpMethod, Resolver as _, Route, RouteMatch};

// The static router (oracle). The `DynResolver` below is built from the identical
// route table so the two must agree on every request.
#[routerama::resolver(name = ApiResolver)]
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

fn dyn_router() -> DynResolver {
    DynResolver::new([
        Route::new(
            "ListBooks",
            HttpMethod::Get,
            PathTemplate::parse("/books", Grammar::default()).expect("valid"),
        ),
        Route::new(
            "CreateBook",
            HttpMethod::Post,
            PathTemplate::parse("/books", Grammar::default()).expect("valid"),
        ),
        Route::new(
            "GetFeatured",
            HttpMethod::Get,
            PathTemplate::parse("/books/featured", Grammar::default()).expect("valid"),
        ),
        Route::new(
            "GetBook",
            HttpMethod::Get,
            PathTemplate::parse("/books/{book}", Grammar::default()).expect("valid"),
        ),
        Route::new(
            "GetReview",
            HttpMethod::Get,
            PathTemplate::parse("/books/{book}/reviews/{review}", Grammar::default()).expect("valid"),
        ),
        Route::new(
            "Archive",
            HttpMethod::Post,
            PathTemplate::parse("/books/{book}:archive", Grammar::default()).expect("valid"),
        ),
        Route::new(
            "Files",
            HttpMethod::Get,
            PathTemplate::parse("/files/**", Grammar::default()).expect("valid"),
        ),
        Route::new(
            "Search",
            HttpMethod::Get,
            PathTemplate::parse("/search", Grammar::default()).expect("valid"),
        ),
    ])
}

// Exotic capture names — a keyword (`type`) and a dotted field path (`shelf.id`)
// — exercise the split between the sanitized Rust *field* identifier and the
// original capture *key*.
#[routerama::resolver(name = ExoticResolver)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ExoticRoute<'p> {
    #[route(GET, "/x/{type}")]
    Typed { _f_type: &'p str },
    #[route(GET, "/shelves/{shelf.id}")]
    Dotted { shelf_id: &'p str },
}

fn exotic_dyn_router() -> DynResolver {
    DynResolver::new([
        Route::new(
            "Typed",
            HttpMethod::Get,
            PathTemplate::parse("/x/{type}", Grammar::default()).expect("valid"),
        ),
        Route::new(
            "Dotted",
            HttpMethod::Get,
            PathTemplate::parse("/shelves/{shelf.id}", Grammar::default()).expect("valid"),
        ),
    ])
}

/// `capture` keys on the variable's *original* name, not the sanitized field
/// identifier, so a keyword (`type`) or dotted (`shelf.id`) name is retrieved by
/// exactly the string written in the template — identically on both backends.
#[test]
fn capture_keys_on_the_original_variable_name() {
    // Static backend: the struct field is a sanitized identifier, but `capture`
    // keys on the original name.
    let typed = ExoticResolver.resolve("GET", "/x/int").expect("match");
    assert_eq!(typed.capture("type"), Some("int"));
    assert_eq!(typed.capture("_f_type"), None); // not the sanitized identifier

    let dotted = ExoticResolver.resolve("GET", "/shelves/sci-fi").expect("match");
    assert_eq!(dotted.capture("shelf.id"), Some("sci-fi"));
    assert_eq!(dotted.capture("shelf_id"), None);

    // Dynamic backend agrees on the same keys.
    let router = exotic_dyn_router();
    assert_eq!(router.resolve("GET", "/x/int").expect("match").capture("type"), Some("int"));
    assert_eq!(
        router.resolve("GET", "/shelves/sci-fi").expect("match").capture("shelf.id"),
        Some("sci-fi")
    );
}

/// Every request resolves to the same route name under the static and dynamic
/// routers — including the precedence-sensitive cases: a literal winning over a
/// wildcard (`/books/featured`), a deeper match winning, a custom verb, method
/// disambiguation, a `**` catch-all, and assorted misses.
#[test]
fn dynamic_agrees_with_static_on_every_request() {
    let router = dyn_router();
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
        let oracle = ApiResolver.resolve(method, path).map(|route| route.name().to_owned());
        let dynamic = router.resolve(method, path).map(|matched| matched.name().to_owned());
        assert_eq!(oracle, dynamic, "static/dynamic disagree on `{method} {path}`");
    }
}

/// Captured path variables match the static router's, by name.
#[test]
fn dynamic_captures_match_the_static_router() {
    let router = dyn_router();

    let review = router.resolve("GET", "/books/rust/reviews/42").expect("match");
    assert_eq!(review.name(), "GetReview");
    assert_eq!(review.capture("book"), Some("rust"));
    assert_eq!(review.capture("review"), Some("42"));
    assert_eq!(review.capture("missing"), None);

    // The static oracle binds the same values into its variant fields.
    match ApiResolver.resolve("GET", "/books/rust/reviews/42").expect("match") {
        ApiRoute::GetReview { book, review } => {
            assert_eq!(book, "rust");
            assert_eq!(review, "42");
        }
        other => panic!("static router matched the wrong route: {other:?}"),
    }

    // The literal `/books/featured` wins over `{book}` and captures nothing.
    let featured = router.resolve("GET", "/books/featured").expect("match");
    assert_eq!(featured.name(), "GetFeatured");
    assert_eq!(featured.captures().count(), 0);
}

/// `resolve` accepts a typed [`HttpMethod`] directly — it implements
/// `AsRef<str>` — through both the static and the dynamic `Resolver`, not just a
/// bare `&str`.
#[test]
fn resolve_accepts_a_typed_http_method() {
    // Static resolver.
    assert!(matches!(
        ApiResolver.resolve(HttpMethod::Post, "/books"),
        Some(ApiRoute::CreateBook)
    ));

    // Static router through the `Resolver` trait.
    let matched = ApiResolver.resolve(HttpMethod::Get, "/books").expect("match");
    assert_eq!(matched.name(), "ListBooks");

    // Dynamic router through the `Resolver` trait.
    let router = dyn_router();
    let dynamic = router.resolve(HttpMethod::Get, "/books/rust").expect("match");
    assert_eq!(dynamic.name(), "GetBook");
    assert_eq!(dynamic.capture("book"), Some("rust"));

    // A custom method carries an arbitrary token and matches on it.
    assert!(router.resolve(HttpMethod::Custom("GET".to_owned()), "/search").is_some());
    assert!(router.resolve(HttpMethod::Delete, "/books").is_none());
}

/// The `Resolver` trait's `resolve` accepts any `AsRef<str>` path (a `String`,
/// `Box<str>`, …), not only `&str`.
#[test]
fn resolve_accepts_a_non_str_path() {
    // Owned `String` path through the dynamic `Resolver` trait; the capture borrows
    // from it.
    let router = dyn_router();
    let owned: String = "/books/rust".to_owned();
    let matched = router.resolve("GET", &owned).expect("match");
    assert_eq!(matched.name(), "GetBook");
    assert_eq!(matched.capture("book"), Some("rust"));

    // `Box<str>` path through the static `Resolver` trait.
    let boxed: Box<str> = "/books".into();
    assert_eq!(ApiResolver.resolve("GET", &boxed).expect("match").name(), "ListBooks");
}

/// An `EitherResolver` blends a static core (wrapped as a dynamic router here) with
/// a dynamic overlay owning a disjoint subtree.
#[test]
fn either_router_blends_disjoint_route_sets() {
    use routerama::EitherResolver;

    let core = dyn_router();
    let plugins = DynResolver::new([
        Route::new(
            "Plugin",
            HttpMethod::Get,
            PathTemplate::parse("/plugins/{name}", Grammar::default()).expect("valid"),
        ),
        Route::new(
            "PluginAction",
            HttpMethod::Post,
            PathTemplate::parse("/plugins/{name}/{action}", Grammar::default()).expect("valid"),
        ),
    ]);
    let router = EitherResolver::new(core, plugins);

    // Core routes still resolve (primary wins).
    assert_eq!(router.resolve("GET", "/books/rust").expect("match").name(), "GetBook");
    // Overlay routes resolve through the fallback, captures intact.
    let plugin = router.resolve("GET", "/plugins/auth").expect("match");
    assert_eq!(plugin.name(), "Plugin");
    assert_eq!(plugin.capture("name"), Some("auth"));
    // A path neither owns is a miss.
    assert!(router.resolve("GET", "/nope").is_none());
}

/// The generated static router (`ApiResolver`, a ZST) implements [`Resolver`],
/// so it resolves through the trait and — the headline goal — composes with a
/// *dynamic* overlay in one [`EitherResolver`]: a zero-cost static core plus a
/// runtime plugin/tenant overlay.
#[test]
fn static_router_composes_with_a_dynamic_overlay() {
    use routerama::EitherResolver;

    // The static router resolves through the `Resolver` trait, and its match
    // exposes captures through `RouteMatch` (by field-name key).
    let matched = ApiResolver.resolve("GET", "/books/rust/reviews/42").expect("match");
    assert_eq!(matched.name(), "GetReview");
    assert_eq!(matched.capture("book"), Some("rust"));
    assert_eq!(matched.capture("review"), Some("42"));
    assert_eq!(matched.capture("missing"), None);

    // Blend the static core with a dynamic plugin overlay owning a disjoint tree.
    let plugins = DynResolver::new([Route::new(
        "Plugin",
        HttpMethod::Get,
        PathTemplate::parse("/plugins/{name}", Grammar::default()).expect("valid"),
    )]);
    let router = EitherResolver::new(ApiResolver, plugins);

    // The static core wins on its own routes...
    assert_eq!(router.resolve("GET", "/books/rust").expect("match").name(), "GetBook");
    // ...and the dynamic overlay handles the rest, captures intact.
    let plugin = router.resolve("GET", "/plugins/auth").expect("match");
    assert_eq!(plugin.name(), "Plugin");
    assert_eq!(plugin.capture("name"), Some("auth"));
    assert!(router.resolve("GET", "/nope").is_none());
}

// A capture-less resolver: every variant is a unit variant, so the enum is *not*
// lifetime-parameterized and `impl<'p> RouteMatch<'p> for Ping` has a free `'p`.
#[routerama::resolver(name = PingResolver)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Ping {
    #[route(GET, "/health")]
    Health,
    #[route(GET, "/ready")]
    Ready,
}

/// The generated ZST router for a capture-less enum (non-lifetime-parameterized)
/// still implements [`Resolver`]/[`RouteMatch`] and composes with a dynamic
/// overlay — pins the free-`'p` `RouteMatch` impl and rvalue-static-promotion of
/// the unit-struct router.
#[test]
fn capture_less_static_router_composes_with_a_dynamic_overlay() {
    use routerama::EitherResolver;

    let matched = PingResolver.resolve("GET", "/health").expect("match");
    assert_eq!(matched.name(), "Health");
    assert_eq!(matched.capture("anything"), None); // no captures ever

    let overlay = DynResolver::new([Route::new(
        "Live",
        HttpMethod::Get,
        PathTemplate::parse("/live/{id}", Grammar::default()).expect("valid"),
    )]);
    let router = EitherResolver::new(PingResolver, overlay);
    assert_eq!(router.resolve("GET", "/ready").expect("match").name(), "Ready");
    let live = router.resolve("GET", "/live/7").expect("match");
    assert_eq!(live.name(), "Live");
    assert_eq!(live.capture("id"), Some("7"));
    assert!(router.resolve("POST", "/health").is_none());
}

/// Generative differential test: synthesize a large space of request paths from a
/// small alphabet of segments (plus custom verbs, empty segments, and varied
/// methods) and assert the static and dynamic routers agree on the resolved name
/// *and* every captured variable for every one. This exercises the two backends
/// against far more inputs — including many misses and precedence collisions —
/// than a curated list can.
#[test]
fn dynamic_agrees_with_static_over_a_generated_path_space() {
    let router = dyn_router();
    // Segments chosen to collide with the route set: literals that also appear as
    // wildcards, the custom-verb suffix, and an empty segment (trailing slash).
    let segments = ["books", "featured", "rust", "reviews", "42", "search", "files", "x", ""];
    let methods = ["GET", "POST", "DELETE"];
    let verbs = ["", ":archive", ":other"];

    let mut checked = 0_u64;
    for method in methods {
        for depth in 0..=4 {
            // Enumerate every `depth`-length combination of segments.
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

                    let oracle = ApiResolver.resolve(method, &full);
                    let dynamic = router.resolve(method, &full);

                    // Names must agree.
                    let oracle_name = oracle.map(|route| route.name().to_owned());
                    let dynamic_name = dynamic.as_ref().map(|m| m.name().to_owned());
                    assert_eq!(oracle_name, dynamic_name, "name disagreement on `{method} {full}`");

                    // Captured variables must agree, by field name.
                    if let (Some(route), Some(matched)) = (oracle, dynamic) {
                        for field in ["book", "review"] {
                            let from_static = static_capture(route, field);
                            let from_dynamic = matched.capture(field);
                            assert_eq!(from_static, from_dynamic, "capture `{field}` disagreement on `{method} {full}`");
                        }
                    }
                    checked += 1;
                }

                // Advance the odometer over `segments`.
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
                        pos = usize::MAX; // signal "wrapped past the end"
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

/// Reads a captured field from a static `ApiRoute` match by its field name, so
/// the generative test can compare captures uniformly with the dynamic router.
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
