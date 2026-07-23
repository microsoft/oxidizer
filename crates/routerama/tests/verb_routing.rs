// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Tests custom-verb handling across static and dynamic routes.

#![allow(
    clippy::literal_string_with_formatting_args,
    reason = "route path templates use `{var}` capture syntax, not string formatting"
)]

use routerama::{HttpMethod, ResolveError, resolver};

#[resolver]
enum Mixed<'p> {
    #[route(GET, "/items/{id}")]
    GetItem {
        id: &'p str,
    },
    Archive {
        id: String,
    },
}

#[test]
fn dynamic_verb_route_is_reachable_despite_overlapping_static_route() {
    let r = Mixed::builder()
        .add_archive(HttpMethod::GET, "/items/{id}:archive")
        .build()
        .expect("mixed static and archive test routes should be valid and non-conflicting");
    match r.resolve("GET", "/items/7:archive") {
        Ok(Mixed::Archive { id }) => assert_eq!(id, "7"),
        _ => panic!("expected Archive; the static route swallowed the verb"),
    }
    match r.resolve("GET", "/items/7") {
        Ok(Mixed::GetItem { id }) => assert_eq!(id, "7"),
        _ => panic!("expected GetItem"),
    }
    assert!(matches!(r.resolve("GET", "/items/7:nope"), Err(ResolveError::NotFound(_))));
}

#[resolver]
enum NoVerbs<'p> {
    #[route(GET, "/refs/{name}")]
    GetRef {
        name: &'p str,
    },
    Plugin {
        name: String,
    },
}

#[test]
fn literal_colon_is_preserved_when_no_route_uses_verbs() {
    let r = NoVerbs::builder()
        .add_plugin(HttpMethod::GET, "/plugins/{name}")
        .build()
        .expect("no-verbs test routes should be valid and non-conflicting");
    match r.resolve("GET", "/refs/abc:def") {
        Ok(NoVerbs::GetRef { name }) => assert_eq!(name, "abc:def"),
        _ => panic!("a literal `:` must be captured, not split, when no route uses verbs"),
    }
    match r.resolve("GET", "/plugins/p:q") {
        Ok(NoVerbs::Plugin { name }) => assert_eq!(name, "p:q"),
        _ => panic!("dynamic side must also preserve a literal `:`"),
    }
}

#[resolver]
enum StaticVerb<'p> {
    #[route(GET, "/docs/{id}:publish")]
    Publish {
        id: &'p str,
    },
    Get {
        id: String,
    },
}

#[test]
fn static_verb_route_forces_dynamic_half_to_split() {
    let r = StaticVerb::builder()
        .add_get(HttpMethod::GET, "/docs/{id}")
        .build()
        .expect("static-verb test routes should be valid and non-conflicting");
    match r.resolve("GET", "/docs/1:publish") {
        Ok(StaticVerb::Publish { id }) => assert_eq!(id, "1"),
        _ => panic!("expected Publish"),
    }
    match r.resolve("GET", "/docs/1") {
        Ok(StaticVerb::Get { id }) => assert_eq!(id, "1"),
        _ => panic!("expected dynamic Get"),
    }
    assert!(matches!(r.resolve("GET", "/docs/1:archive"), Err(ResolveError::NotFound(_))));
}
