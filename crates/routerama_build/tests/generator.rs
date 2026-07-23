// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Generator public API tests.

#![cfg(feature = "codegen")]

use http_path_template::{Grammar, PathTemplate};
use quote::quote;
use routerama_build::{Generator, Route};

fn rule(name: &str, template: &str) -> Route {
    Route::new(
        name,
        "GET",
        PathTemplate::parse(template, Grammar::default()).expect("valid template"),
    )
}

#[test]
fn add_appends_a_route_to_the_generated_output() {
    let mut generator = Generator::new("Route", true);
    generator.add(rule("GetShelf", "/v1/shelves/{shelf}"));
    let code = generator.generate().to_string();
    assert!(code.contains("GetShelf"), "add must include the route: {code}");
}

#[test]
fn add_all_appends_every_route() {
    let mut generator = Generator::new("Route", true);
    generator.add_all([rule("ListShelves", "/v1/shelves"), rule("GetShelf", "/v1/shelves/{shelf}")]);
    let code = generator.generate().to_string();
    assert!(
        code.contains("ListShelves") && code.contains("GetShelf"),
        "add_all must include every route: {code}"
    );
}

#[test]
fn private_visibility_omits_pub() {
    let mut generator = Generator::new("Route", false);
    generator.add(rule("GetShelf", "/v1/shelves/{shelf}"));
    let code = generator.generate().to_string();
    assert!(!code.contains("pub enum"), "a private generator emits no `pub`: {code}");
}

#[test]
fn runtime_path_can_be_overridden() {
    let mut generator = Generator::new("Route", false);
    generator
        .runtime_path(quote! { ::renamed::codegen_helpers })
        .add(rule("GetShelf", "/v1/shelves/{shelf}"));
    let code = generator.generate().to_string();
    assert!(code.contains("renamed"), "{code}");
    assert!(!code.contains(":: routerama :: codegen_helpers"), "{code}");
}

#[test]
fn minimal_api_omits_unused_raw_enum_impls() {
    let mut generator = Generator::new("Route", false);
    generator.full_api(false).add(rule("GetShelf", "/v1/shelves/{shelf}"));
    let code = generator.generate().to_string();
    assert!(code.contains("fn resolve"), "{code}");
    assert!(!code.contains("RouteMatch"), "{code}");
    assert!(!code.contains("derive"), "{code}");
}

#[test]
fn unicode_xid_route_identifiers_generate_valid_rust() {
    let mut generator = Generator::new("Διαδρομή", true);
    generator.add(rule("路由", "/unicode"));
    let code = generator.generate();
    syn::parse2::<syn::File>(code).expect("Rust Unicode XID names are valid generated identifiers");
}

#[test]
fn route_name_is_not_reserved_by_a_trait_method() {
    let mut generator = Generator::new("Route", true);
    generator.add(rule("name", "/name"));
    let code = generator.generate();
    let rendered = code.to_string();
    assert!(!rendered.contains("compile_error"), "{rendered}");
    syn::parse2::<syn::File>(code).expect("trait method names do not collide with enum variants");
}
