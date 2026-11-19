// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Tests of the route table of runtime resolvers.

#![allow(clippy::unwrap_used, reason = "test code")]

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

/// Matches `method` and `path`, and returns the template of the route and the value of the parameter `name`. It takes
/// a `dyn RouteTable`, as a telemetry layer keeps one.
fn hit<'m, 'p>(table: &'m dyn RouteTable, method: &str, path: &'p str, name: &str) -> Option<(&'m str, Option<&'p str>)> {
    let hit = table.match_route(method, path)?;
    Some((hit.template().as_ref(), hit.parameter(name)))
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
        [
            ("GET", "/books/rust"),
            ("POST", "/books/rust:archive"),
            ("GET", "/files/a/b"),
            ("GET", "/static/a/b"),
            ("GET", "/img-cat.png"),
        ]
        .map(|(method, path)| resolver.match_route(method, path).map(|hit| hit.template().as_ref())),
        [
            Some("/books/{book}"),
            Some("/books/{book}:archive"),
            Some("/files/{path=**}"),
            Some("/static/**"),
            Some("/img-{id}.png"),
        ]
    );
}

#[test]
fn parameter_finds_value_by_template_name() {
    let resolver = table(&[
        ("GET", "/v1/{a}/{b}/{c}/{d}/{e}"),
        ("GET", "/shelves/{shelf.id}/books/{book}"),
        ("GET", "/files/{dir}/{path=**}"),
        ("GET", "/img-{id}.png"),
    ]);

    assert_eq!(
        [
            ("/v1/1/2/3/4/5", "e"),
            ("/shelves/7/books/rust", "shelf.id"),
            ("/shelves/7/books/rust", "book"),
            ("/shelves/7/books/rust", "shelf"),
            ("/files/docs/a/b", "dir"),
            ("/files/docs/a/b", "path"),
            ("/img-cat.png", "id"),
        ]
        .map(|(path, name)| resolver.match_route("GET", path).unwrap().parameter(name)),
        [Some("5"), Some("7"), Some("rust"), None, Some("docs"), Some("a/b"), Some("cat")]
    );
}

#[test]
fn dyn_builder_resolver_matches_its_routes() {
    let mut builder = DynBuilder::new();
    builder.add(HttpMethod::GET, "/plugins/{name}", &["name"], "Plugin", ());
    builder.add(HttpMethod::POST, "/shelves/{shelf.id}/books", &["shelf.id"], "Shelf", ());
    let resolver = builder.finish().unwrap();

    assert_eq!(
        [("GET", "/plugins/auth", "name"), ("POST", "/shelves/7/books", "shelf.id")]
            .map(|(method, path, name)| hit(&resolver, method, path, name)),
        [
            Some(("/plugins/{name}", Some("auth"))),
            Some(("/shelves/{shelf.id}/books", Some("7")))
        ]
    );
}

#[test]
fn match_route_returns_raw_percent_encoded_parameters() {
    let resolver = table(&[("GET", "/books/{book}/reviews/{review}")]);

    let hit = resolver.match_route("GET", "/books/r%C3%BCst/reviews/a%2Fb").unwrap();

    assert_eq!([hit.parameter("book"), hit.parameter("review")], [Some("r%C3%BCst"), Some("a%2Fb")]);
}

#[test]
fn match_route_gives_matched_template() {
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
    .map(|(method, path)| resolver.match_route(method, path).map(|hit| hit.template().as_ref()))
    .collect();

    assert_eq!(
        templates,
        [Some("/books"), Some("/books/{book}"), Some("/books/featured"), Some("/authors")]
    );
}

#[test]
fn match_route_misses_return_none() {
    let resolver = table(&[("GET", "/books/{book}")]);

    let templates: Vec<Option<&str>> = [("POST", "/books/rust"), ("GET", "/authors/rust"), ("GET", "/books")]
        .iter()
        .map(|(method, path)| resolver.match_route(method, path).map(|hit| hit.template().as_ref()))
        .collect();

    assert_eq!(templates, [None, None, None]);
}

#[test]
fn match_route_rejects_query_and_fragment() {
    let resolver = table(&[("GET", "/books/{book}")]);

    let templates: Vec<Option<&str>> = ["/books/rust?x=1", "/books/rust#top"]
        .iter()
        .map(|path| resolver.match_route("GET", path).map(|hit| hit.template().as_ref()))
        .collect();

    assert_eq!(templates, [None, None]);
}
