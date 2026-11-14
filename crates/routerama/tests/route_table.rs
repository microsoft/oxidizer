// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Tests of the route table of runtime resolvers.

#![allow(clippy::unwrap_used, reason = "test code")]

use std::fmt::Write as _;

use http_path_template::{Grammar, PathTemplate};
use routerama::__rt::{DynBuilder, RawResolver, Route};
use routerama::{HttpMethod, RouteTable};

/// Builds a resolver from `(method, template)` pairs.
fn table(routes: &[(&str, &str)]) -> RawResolver {
    RawResolver::new(routes.iter().enumerate().map(|(index, (method, template))| {
        Route::new(
            format!("Route{index}"),
            *method,
            PathTemplate::parse(template, Grammar::default().with_segment_affixes()).unwrap(),
        )
    }))
}

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

#[test]
fn raw_resolver_lists_every_route() {
    let resolver = table(&[("GET", "/books"), ("GET", "/books/{book}"), ("POST", "/authors")]);

    assert_eq!(
        listing(&resolver),
        [("/authors", vec![]), ("/books", vec![]), ("/books/{book}", vec!["book"])]
    );
}

#[test]
fn template_reports_template_text() {
    let resolver = table(&[
        ("GET", "/books/{book}"),
        ("POST", "/books/{book}:archive"),
        ("GET", "/files/{path=**}"),
        ("GET", "/static/**"),
        ("GET", "/img-{id}.png"),
    ]);

    assert_eq!(
        listing(&resolver),
        [
            ("/books/{book}", vec!["book"]),
            ("/books/{book}:archive", vec!["book"]),
            ("/files/{path=**}", vec!["path"]),
            ("/img-{id}.png", vec!["id"]),
            ("/static/**", vec![]),
        ]
    );
}

#[test]
fn parameters_follow_template_order() {
    let resolver = table(&[("GET", "/shelves/{shelf}/books/{book}")]);

    assert_eq!(listing(&resolver), [("/shelves/{shelf}/books/{book}", vec!["shelf", "book"])]);
}

#[test]
fn dotted_capture_names_are_joined_with_dots() {
    let resolver = table(&[("GET", "/shelves/{shelf.id}/books")]);

    assert_eq!(listing(&resolver), [("/shelves/{shelf.id}/books", vec!["shelf.id"])]);
}

#[test]
fn dyn_builder_resolver_lists_its_routes() {
    let mut builder = DynBuilder::new();
    builder.add(HttpMethod::GET, "/plugins/{name}", &["name"], "Plugin", ());
    builder.add(HttpMethod::POST, "/shelves/{shelf.id}/books", &["shelf.id"], "Shelf", ());
    let resolver = builder.finish().unwrap();

    assert_eq!(
        listing(&resolver),
        [("/plugins/{name}", vec!["name"]), ("/shelves/{shelf.id}/books", vec!["shelf.id"])]
    );
}

#[test]
fn match_route_returns_raw_percent_encoded_captures() {
    let resolver = table(&[("GET", "/books/{book}/reviews/{review}")]);

    let hit = resolver.match_route("GET", "/books/r%C3%BCst/reviews/a%2Fb").unwrap();

    assert_eq!(hit.captures(), ["r%C3%BCst", "a%2Fb"]);
}

#[test]
fn match_route_number_gives_matched_template() {
    let resolver = table(&[
        ("GET", "/books"),
        ("GET", "/books/{book}"),
        ("GET", "/books/featured"),
        ("POST", "/authors"),
    ]);

    let templates: Vec<Option<&str>> = [
        ("GET", "/books"),
        ("GET", "/books/rust"),
        ("GET", "/books/featured"),
        ("POST", "/authors"),
    ]
    .iter()
    .map(|(method, path)| {
        resolver
            .match_route(method, path)
            .map(|hit| resolver.template(hit.route()).unwrap())
    })
    .collect();

    assert_eq!(
        templates,
        [Some("/books"), Some("/books/{book}"), Some("/books/featured"), Some("/authors")]
    );
}

#[test]
fn capture_count_matches_parameter_count() {
    let resolver = table(&[
        ("GET", "/v1/{a}/{b}/{c}/{d}/{e}"),
        ("GET", "/files/{dir}/{path=**}"),
        ("GET", "/img-{id}.png"),
    ]);

    let names_and_captures: Vec<(Vec<&str>, Vec<&str>)> = ["/v1/1/2/3/4/5", "/files/docs/a/b", "/img-cat.png"]
        .iter()
        .map(|path| {
            let hit = resolver.match_route("GET", path).unwrap();
            let names = resolver.parameters(hit.route()).unwrap().iter().map(AsRef::as_ref).collect();
            (names, hit.captures().to_vec())
        })
        .collect();

    assert_eq!(
        names_and_captures,
        [
            (vec!["a", "b", "c", "d", "e"], vec!["1", "2", "3", "4", "5"]),
            (vec!["dir", "path"], vec!["docs", "a/b"]),
            (vec!["id"], vec!["cat"]),
        ]
    );
}

#[test]
fn match_route_misses_return_none() {
    let resolver = table(&[("GET", "/books/{book}")]);

    let routes: Vec<Option<usize>> = [("POST", "/books/rust"), ("GET", "/authors/rust"), ("GET", "/books")]
        .iter()
        .map(|(method, path)| resolver.match_route(method, path).map(|hit| hit.route()))
        .collect();

    assert_eq!(routes, [None, None, None]);
}

#[test]
fn match_route_rejects_query_and_fragment() {
    let resolver = table(&[("GET", "/books/{book}")]);

    let routes: Vec<Option<usize>> = ["/books/rust?x=1", "/books/rust#top"]
        .iter()
        .map(|path| resolver.match_route("GET", path).map(|hit| hit.route()))
        .collect();

    assert_eq!(routes, [None, None]);
}

#[test]
fn unknown_route_number_returns_none() {
    let resolver = table(&[("GET", "/books")]);

    assert_eq!((resolver.template(1), resolver.parameters(1)), (None, None));
}

#[test]
#[cfg_attr(
    miri,
    ignore = "insta reads snapshot files; the other listing tests in this file check the same data under Miri"
)]
fn route_listing_prints_templates_and_parameters() {
    let resolver = table(&[
        ("GET", "/books"),
        ("GET", "/books/{book}"),
        ("GET", "/shelves/{shelf.id}/books/{book}/reviews/{review}"),
        ("GET", "/files/{path=**}"),
    ]);

    let mut printed = String::new();
    for (template, parameters) in listing(&resolver) {
        writeln!(printed, "{template} [{}]", parameters.join(", ")).unwrap();
    }

    insta::assert_snapshot!(printed);
}
