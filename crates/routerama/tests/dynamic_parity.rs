// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Differential tests: the runtime [`DynRouter`] must resolve a route set
//! *identically* to the static `routes!`-generated router, since both walk the
//! same shared trie. The static router is the oracle.

#![cfg(all(feature = "dynamic", feature = "macros"))]

use routerama::{DynRouter, HttpMethod, RouteMatch, RouteRule, Router as _};

// The static router (oracle). The `DynRouter` below is built from the identical
// route table so the two must agree on every request.
routerama::routes! {
    enum ApiRoute {
        ListBooks   GET    "/books",
        CreateBook  POST   "/books",
        GetFeatured GET    "/books/featured",
        GetBook     GET    "/books/{book}",
        GetReview   GET    "/books/{book}/reviews/{review}",
        Archive     POST   "/books/{book}:archive",
        Files       GET    "/files/**",
        Search      GET    "/search",
    }
    struct ApiRouter;
}

fn dyn_router() -> DynRouter {
    DynRouter::new([
        RouteRule::new("ListBooks", HttpMethod::Get, "/books".parse().expect("valid")),
        RouteRule::new("CreateBook", HttpMethod::Post, "/books".parse().expect("valid")),
        RouteRule::new("GetFeatured", HttpMethod::Get, "/books/featured".parse().expect("valid")),
        RouteRule::new("GetBook", HttpMethod::Get, "/books/{book}".parse().expect("valid")),
        RouteRule::new(
            "GetReview",
            HttpMethod::Get,
            "/books/{book}/reviews/{review}".parse().expect("valid"),
        ),
        RouteRule::new("Archive", HttpMethod::Post, "/books/{book}:archive".parse().expect("valid")),
        RouteRule::new("Files", HttpMethod::Get, "/files/**".parse().expect("valid")),
        RouteRule::new("Search", HttpMethod::Get, "/search".parse().expect("valid")),
    ])
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
        let oracle = ApiRoute::resolve(method, path).map(|route| route.name().to_owned());
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
    match ApiRoute::resolve("GET", "/books/rust/reviews/42").expect("match") {
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

/// An `EitherRouter` blends a static core (wrapped as a dynamic router here) with
/// a dynamic overlay owning a disjoint subtree.
#[test]
fn either_router_blends_disjoint_route_sets() {
    use routerama::EitherRouter;

    let core = dyn_router();
    let plugins = DynRouter::new([
        RouteRule::new("Plugin", HttpMethod::Get, "/plugins/{name}".parse().expect("valid")),
        RouteRule::new("PluginAction", HttpMethod::Post, "/plugins/{name}/{action}".parse().expect("valid")),
    ]);
    let router = EitherRouter::new(core, plugins);

    // Core routes still resolve (primary wins).
    assert_eq!(router.resolve("GET", "/books/rust").expect("match").name(), "GetBook");
    // Overlay routes resolve through the fallback, captures intact.
    let plugin = router.resolve("GET", "/plugins/auth").expect("match");
    assert_eq!(plugin.name(), "Plugin");
    assert_eq!(plugin.capture("name"), Some("auth"));
    // A path neither owns is a miss.
    assert!(router.resolve("GET", "/nope").is_none());
}

/// The generated static router (`ApiRouter`, a ZST) implements [`Router`],
/// so it resolves through the trait and — the headline goal — composes with a
/// *dynamic* overlay in one [`EitherRouter`]: a zero-cost static core plus a
/// runtime plugin/tenant overlay.
#[test]
fn static_router_composes_with_a_dynamic_overlay() {
    use routerama::EitherRouter;

    // The static router resolves through the `Router` trait, and its match
    // exposes captures through `RouteMatch` (by field-name key).
    let matched = ApiRouter.resolve("GET", "/books/rust/reviews/42").expect("match");
    assert_eq!(matched.name(), "GetReview");
    assert_eq!(matched.capture("book"), Some("rust"));
    assert_eq!(matched.capture("review"), Some("42"));
    assert_eq!(matched.capture("missing"), None);

    // Blend the static core with a dynamic plugin overlay owning a disjoint tree.
    let plugins = DynRouter::new([RouteRule::new("Plugin", HttpMethod::Get, "/plugins/{name}".parse().expect("valid"))]);
    let router = EitherRouter::new(ApiRouter, plugins);

    // The static core wins on its own routes...
    assert_eq!(router.resolve("GET", "/books/rust").expect("match").name(), "GetBook");
    // ...and the dynamic overlay handles the rest, captures intact.
    let plugin = router.resolve("GET", "/plugins/auth").expect("match");
    assert_eq!(plugin.name(), "Plugin");
    assert_eq!(plugin.capture("name"), Some("auth"));
    assert!(router.resolve("GET", "/nope").is_none());
}

// A capture-less router: every variant is a unit variant, so the enum is *not*
// lifetime-parameterized and `impl<'p> RouteMatch<'p> for Ping` has a free `'p`.
routerama::routes! {
    enum Ping {
        Health GET "/health",
        Ready  GET "/ready",
    }
    struct PingRouter;
}

/// The generated ZST router for a capture-less enum (non-lifetime-parameterized)
/// still implements [`Router`]/[`RouteMatch`] and composes with a dynamic
/// overlay — pins the free-`'p` `RouteMatch` impl and rvalue-static-promotion of
/// the unit-struct router.
#[test]
fn capture_less_static_router_composes_with_a_dynamic_overlay() {
    use routerama::EitherRouter;

    let matched = PingRouter.resolve("GET", "/health").expect("match");
    assert_eq!(matched.name(), "Health");
    assert_eq!(matched.capture("anything"), None); // no captures ever

    let overlay = DynRouter::new([RouteRule::new("Live", HttpMethod::Get, "/live/{id}".parse().expect("valid"))]);
    let router = EitherRouter::new(PingRouter, overlay);
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

                    let oracle = ApiRoute::resolve(method, &full);
                    let dynamic = router.resolve(method, &full);

                    // Names must agree.
                    let oracle_name = oracle.map(|route| route.name());
                    let dynamic_name = dynamic.as_ref().map(RouteMatch::name);
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
