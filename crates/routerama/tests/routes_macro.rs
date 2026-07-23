// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Behavioral tests for the `#[resolver]` macro.

use routerama::{ResolveError, Resolver, resolver};

#[resolver]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ApiRoute<'p> {
    #[route(GET, "/books")]
    ListBooks,
    #[route(POST, "/books")]
    CreateBook,
    #[route(GET, "/books/{book}")]
    GetBook { book: &'p str },
    #[route(GET, "/books/{book}/reviews/{review}")]
    GetReview { book: &'p str, review: &'p str },
    #[route(GET, "/items/{item.id}")]
    GetItem { item_id: &'p str },
    #[route(POST, "/books/{book}:archive")]
    ArchiveBook { book: &'p str },
    #[route(GET, "/search")]
    SearchBooks,
}

#[test]
fn unit_route_has_no_captures() {
    assert_eq!(api().resolve("GET", "/books"), Ok(ApiRoute::ListBooks));
}

#[test]
fn method_disambiguates_same_path() {
    assert_eq!(api().resolve("GET", "/books"), Ok(ApiRoute::ListBooks));
    assert_eq!(api().resolve("POST", "/books"), Ok(ApiRoute::CreateBook));
}

#[test]
fn single_capture_is_read_from_the_variant_field() {
    assert_eq!(api().resolve("GET", "/books/rust"), Ok(ApiRoute::GetBook { book: "rust" }));
}

#[test]
fn multiple_captures() {
    assert_eq!(
        api().resolve("GET", "/books/rust/reviews/42"),
        Ok(ApiRoute::GetReview {
            book: "rust",
            review: "42"
        })
    );
}

#[test]
fn dotted_capture_becomes_an_underscore_field() {
    assert_eq!(api().resolve("GET", "/items/99"), Ok(ApiRoute::GetItem { item_id: "99" }));
}

#[test]
fn custom_verb_selects_a_distinct_route() {
    assert_eq!(
        api().resolve("POST", "/books/rust:archive"),
        Ok(ApiRoute::ArchiveBook { book: "rust" })
    );
    assert!(matches!(api().resolve("POST", "/books/rust"), Err(ResolveError::NotFound(_))));
}

#[test]
fn misses_resolve_to_not_found() {
    assert!(matches!(api().resolve("DELETE", "/books"), Err(ResolveError::NotFound(_))));
    assert!(matches!(api().resolve("GET", "/unknown"), Err(ResolveError::NotFound(_))));
    assert!(matches!(api().resolve("GET", "/books/rust/extra"), Err(ResolveError::NotFound(_))));
}

#[test]
fn query_and_fragment_delimiters_are_invalid_paths() {
    for path in [
        "/books?sort=title",
        "/books#reviews",
        "/books/rust:archive?force",
        "/books/rust:archive#result",
    ] {
        assert_eq!(api().resolve("GET", path), Err(ResolveError::InvalidPath(path)));
        assert_eq!(explicit_resolver().resolve("GET", path), Err(ResolveError::InvalidPath(path)));
    }
}

#[test]
fn not_found_carries_the_unmatched_path() {
    assert_eq!(
        api().resolve("GET", "/no/such/route"),
        Err(ResolveError::NotFound("/no/such/route"))
    );
    match api().resolve("DELETE", "/books") {
        Err(ResolveError::NotFound(path)) => assert_eq!(path, "/books"),
        other => panic!("expected NotFound, got {other:?}"),
    }
}

#[resolver]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HealthRoute {
    #[route(GET, "/health")]
    Health,
}

#[resolver]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ErrorNameRoutes {
    #[route(GET, "/not-found")]
    NotFound,
    #[route(GET, "/invalid-capture")]
    InvalidCapture,
}

#[resolver(name = ExplicitResolver)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ExplicitRoute {
    #[route(GET, "/explicit")]
    Static,
    Dynamic,
}

#[test]
fn explicit_resolver_name_is_used_for_the_resolver_and_builder() {
    let resolver = explicit_resolver();
    assert_eq!(resolver.resolve("GET", "/explicit"), Ok(ExplicitRoute::Static));
    assert_eq!(resolver.resolve("GET", "/dynamic"), Ok(ExplicitRoute::Dynamic));
}

fn explicit_resolver() -> ExplicitResolver {
    explicit_resolver_builder().build().expect("explicitly named resolver builds")
}

fn explicit_resolver_builder() -> ExplicitResolverBuilder {
    ExplicitRoute::builder().add_dynamic(routerama::HttpMethod::GET, "/dynamic")
}

#[test]
fn captureless_enum_keeps_the_not_found_path_in_the_error() {
    assert_eq!(health().resolve("GET", "/health"), Ok(HealthRoute::Health));
    assert_eq!(health().resolve("GET", "/nope"), Err(ResolveError::NotFound("/nope")));
}

#[test]
fn resolution_error_names_are_available_as_domain_variants() {
    let resolver = ErrorNameRoutes::resolver();
    assert_eq!(resolver.resolve("GET", "/not-found"), Ok(ErrorNameRoutes::NotFound));
    assert_eq!(resolver.resolve("GET", "/invalid-capture"), Ok(ErrorNameRoutes::InvalidCapture));
}

#[test]
fn resolves_the_expected_variant() {
    assert!(matches!(api().resolve("GET", "/search"), Ok(ApiRoute::SearchBooks)));
}

#[resolver]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum UserRoute<'p> {
    #[route(GET, "/users")]
    ListUsers,
    #[route(GET, "/users/{user}")]
    GetUser { user: &'p str },
}

#[test]
fn two_resolvers_coexist_in_the_same_scope() {
    assert_eq!(user().resolve("GET", "/users/ada"), Ok(UserRoute::GetUser { user: "ada" }));
    assert!(matches!(user().resolve("GET", "/books/rust"), Err(ResolveError::NotFound(_))));
    assert!(matches!(api().resolve("GET", "/users/ada"), Err(ResolveError::NotFound(_))));
}

#[test]
fn builder_produces_the_concrete_resolver_type() {
    let resolver = api();
    assert_eq!(resolver.resolve("GET", "/books/rust"), Ok(ApiRoute::GetBook { book: "rust" }));
    assert!(format!("{resolver:?}").contains("ApiRouteResolver"));
}

fn resolve_get<'p, R: Resolver>(resolver: &R, path: &'p str) -> Result<R::Route<'p>, ResolveError<'p>> {
    resolver.resolve("GET", path)
}

#[test]
fn resolvers_support_generic_dispatch() {
    assert_eq!(resolve_get(&ApiRoute::resolver(), "/books"), Ok(ApiRoute::ListBooks));
}

#[resolver]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MethodRoute<'p> {
    #[route(PUT, "/items/{item}")]
    Replace { item: &'p str },
    #[route(DELETE, "/items/{item}")]
    Remove { item: &'p str },
    #[route(PATCH, "/items/{item}")]
    Tweak { item: &'p str },
    #[route(PURGE, "/items/{item}")]
    Purge { item: &'p str },
    #[route("M-SEARCH", "/items/{item}")]
    Search { item: &'p str },
}

#[test]
fn every_method_token_resolves_its_route() {
    assert_eq!(method_route().resolve("PUT", "/items/1"), Ok(MethodRoute::Replace { item: "1" }));
    assert_eq!(method_route().resolve("DELETE", "/items/1"), Ok(MethodRoute::Remove { item: "1" }));
    assert_eq!(method_route().resolve("PATCH", "/items/1"), Ok(MethodRoute::Tweak { item: "1" }));
    assert_eq!(method_route().resolve("PURGE", "/items/1"), Ok(MethodRoute::Purge { item: "1" }));
    assert_eq!(
        method_route().resolve("M-SEARCH", "/items/1"),
        Ok(MethodRoute::Search { item: "1" })
    );
}

#[resolver]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum NameRoute<'p> {
    #[route(GET, "/users/{name}")]
    GetUser { name: &'p str },
    #[route(GET, "/tags/{name}/{value}")]
    GetTag { name: &'p str, value: &'p str },
}

#[test]
fn captured_field_named_name_resolves() {
    assert_eq!(name_route().resolve("GET", "/users/ada"), Ok(NameRoute::GetUser { name: "ada" }));
    assert_eq!(
        name_route().resolve("GET", "/tags/rust/1.0"),
        Ok(NameRoute::GetTag {
            name: "rust",
            value: "1.0"
        })
    );
}

#[resolver]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum KeyRoute<'p> {
    #[route(GET, "/things/{__key}")]
    GetThing { __key: &'p str },
}

#[test]
fn captured_field_named_like_the_capture_key_parameter_resolves() {
    assert_eq!(
        key_route().resolve("GET", "/things/widget"),
        Ok(KeyRoute::GetThing { __key: "widget" })
    );
}

#[resolver]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MultiRoute<'p> {
    #[route(GET, "/status")]
    #[route(HEAD, "/status")]
    Status,
    #[route(GET, "/users/{user}")]
    #[route(GET, "/accounts/{user}")]
    User { user: &'p str },
}

#[test]
fn multiple_route_attributes_bind_one_variant_to_each() {
    assert_eq!(multi().resolve("GET", "/status"), Ok(MultiRoute::Status));
    assert_eq!(multi().resolve("HEAD", "/status"), Ok(MultiRoute::Status));
    assert!(matches!(multi().resolve("POST", "/status"), Err(ResolveError::NotFound(_))));

    assert_eq!(multi().resolve("GET", "/users/alice"), Ok(MultiRoute::User { user: "alice" }));
    assert_eq!(multi().resolve("GET", "/accounts/bob"), Ok(MultiRoute::User { user: "bob" }));
}

fn api() -> ApiRouteResolver {
    ApiRoute::resolver()
}
fn health() -> HealthRouteResolver {
    HealthRoute::resolver()
}
fn user() -> UserRouteResolver {
    UserRoute::resolver()
}
fn method_route() -> MethodRouteResolver {
    MethodRoute::resolver()
}
fn name_route() -> NameRouteResolver {
    NameRoute::resolver()
}
fn key_route() -> KeyRouteResolver {
    KeyRoute::resolver()
}
fn multi() -> MultiRouteResolver {
    MultiRoute::resolver()
}
