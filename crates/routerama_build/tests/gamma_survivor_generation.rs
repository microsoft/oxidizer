// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Generated syntax, diagnostics, and route collection regression oracles.

#![cfg(feature = "codegen")]

use http_path_template::{Grammar, PathTemplate};
use quote::quote;
use routerama_build::{Generator, Route};

#[cfg(test)]
fn route(name: &str, path: &str) -> Route {
    Route::new(
        name,
        "GET",
        PathTemplate::parse(path, Grammar::default().with_segment_affixes()).unwrap(),
    )
}

#[test]
fn gamma_survivor_generator_invalid_names_and_conflicting_keys() {
    for (name, path, detail) in [
        ("bad-name", "/x", "not a valid Rust identifier"),
        ("resolve", "/x", "collides with a generated method"),
        ("Duplicate", "/{a.b}/{a_b}", "same field"),
    ] {
        let mut generator = Generator::new("Routes", true);
        generator.add(route(name, path));
        let output = generator.generate().to_string();
        assert!(output.contains("compile_error"), "{output}");
        assert!(output.contains(detail), "{output}");
    }
    let mut generator = Generator::new("bad-name", false);
    generator.add(route("Home", "/"));
    assert!(generator.generate().to_string().contains("enum __InvalidRouteramaRouteType"));
    assert!(
        generator
            .generate()
            .to_string()
            .contains("generated route type name `bad-name` is not a valid Rust identifier")
    );
    let mut generator = Generator::new("Routes", true);
    generator.add_all([route("Same", "/{a}"), route("Same", "/other")]);
    assert!(
        generator
            .generate()
            .to_string()
            .contains("capture different path variables ({a} vs none)")
    );
    let mut generator = Generator::new("Routes", true);
    generator.add_all([route("First", "/same"), route("Second", "/same")]);
    assert!(generator.generate().to_string().contains("conflicting routes"));
}

#[test]
fn gamma_survivor_generator_route_order_aliases_and_api_flags() {
    for count in [3, 1024] {
        let mut generator = Generator::new("Routes", true);
        generator.add_all((0..count).map(|i| route(&format!("Route{i}"), &format!("/r{i}"))));
        generator.add(route("Route0", "/alias"));
        generator.add(route(&format!("Route{}", count - 1), "/last-alias"));
        let file = syn::parse2::<syn::File>(generator.generate()).unwrap();
        let enumeration = file
            .items
            .iter()
            .find_map(|item| match item {
                syn::Item::Enum(item) => Some(item),
                _ => None,
            })
            .unwrap();
        assert!(matches!(enumeration.vis, syn::Visibility::Public(_)));
        assert_eq!(enumeration.variants.len(), count);
        assert_eq!(enumeration.variants.first().unwrap().ident, "Route0");
        assert_eq!(enumeration.variants.last().unwrap().ident, format!("Route{}", count - 1));
        assert!(enumeration.generics.params.is_empty());
    }
    let mut generator = Generator::new("Routes", false);
    generator.full_api(false).runtime_path(quote! { ::custom::runtime }).add_all([
        route("Span", "/{id=shelves/*}"),
        route("Rest", "/files/{path=**}"),
        route("Affix", "/img-{name}.png"),
        route("Single", "/plain/{name}"),
    ]);
    let output = generator.generate();
    syn::parse2::<syn::File>(output.clone()).unwrap();
    let output = output.to_string();
    for token in [":: custom :: runtime", "capture (0 , 1)", "rest (1)", "affix (0 , 4 , 4)"] {
        assert!(output.contains(token), "{token}: {output}");
    }
    assert!(!output.contains("RouteMatch"));
    assert!(!output.contains("pub enum"));
    assert!(output.contains("enum Routes < 'p >"));
    let mut generator = Generator::new("PairRoutes", true);
    generator.add_all([route("Pair", "/{a}/{z}"), route("Pair", "/alias/{z}/{a}")]);
    let output = generator.generate();
    assert!(!output.to_string().contains("compile_error"), "{output}");
    let file = syn::parse2::<syn::File>(output).unwrap();
    let enumeration = file
        .items
        .iter()
        .find_map(|item| match item {
            syn::Item::Enum(item) => Some(item),
            _ => None,
        })
        .unwrap();
    assert_eq!(enumeration.variants.len(), 1);
    assert_eq!(
        enumeration.variants[0]
            .fields
            .iter()
            .map(|field| field.ident.as_ref().unwrap().to_string())
            .collect::<Vec<_>>(),
        ["a", "z"]
    );
}
