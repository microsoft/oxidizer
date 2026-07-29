// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Procedural macro expansion public API tests.

#![cfg(feature = "codegen")]

use quote::quote;
use routerama_build::macro_impl::{derive_from_query, derive_to_query, resolver};

#[test]
fn from_query_expands_a_decoder() {
    let expanded = derive_from_query(quote! {
        struct Request {
            value: String,
        }
    })
    .to_string();

    assert!(expanded.contains("DecodeFields"), "{expanded}");
}

#[test]
fn to_query_expands_an_encoder() {
    let expanded = derive_to_query(quote! {
        struct Request {
            value: String,
        }
    })
    .to_string();

    assert!(expanded.contains("EncodeFields"), "{expanded}");
}

#[test]
fn query_derives_preserve_generics_and_where_clauses() {
    let input = quote! {
        struct Request<'q, 'marker, T, const N: usize>
        where
            T: Clone,
        {
            value: &'q str,
            generic: T,
            #[query(skip)]
            marker: core::marker::PhantomData<(&'marker (), [T; N])>,
        }
    };
    let decoded = derive_from_query(input.clone()).to_string();
    let encoded = derive_to_query(input).to_string();

    for expanded in [&decoded, &encoded] {
        assert!(expanded.contains("const N : usize"), "{expanded}");
        assert!(expanded.contains("T : Clone"), "{expanded}");
        assert!(expanded.contains("'marker"), "{expanded}");
    }
    assert!(decoded.contains("T : :: core :: str :: FromStr"), "{decoded}");
    assert!(encoded.contains("T : :: core :: fmt :: Display"), "{encoded}");
}

#[test]
fn static_resolver_expands_an_infallible_constructor_without_a_builder() {
    let expanded = resolver(
        quote! {},
        quote! {
            enum Route {
                #[route(GET, "/")]
                Home,
            }
        },
    )
    .to_string();

    assert!(expanded.contains("fn resolver"), "{expanded}");
    assert!(expanded.contains("RouteResolver"), "{expanded}");
    assert!(!expanded.contains("RouteResolverBuilder"), "{expanded}");
}

#[test]
fn dynamic_resolver_expands_a_builder() {
    let expanded = resolver(
        quote! {},
        quote! {
            enum Route {
                Home,
            }
        },
    )
    .to_string();

    assert!(expanded.contains("RouteResolver"), "{expanded}");
    assert!(expanded.contains("RouteResolverBuilder"), "{expanded}");
    assert!(expanded.contains("fn builder"), "{expanded}");
}

#[test]
fn resolver_accepts_an_explicit_type_name() {
    let expanded = resolver(
        quote! { name = ApiResolver },
        quote! {
            enum Route {
                Home,
            }
        },
    )
    .to_string();

    assert!(expanded.contains("struct ApiResolver"), "{expanded}");
    assert!(expanded.contains("struct ApiResolverBuilder"), "{expanded}");
}

#[test]
fn resolver_rejects_unknown_naming_arguments() {
    let expanded = resolver(
        quote! { type_name = ApiResolver },
        quote! {
            enum Route {
                #[route(GET, "/")]
                Home,
            }
        },
    )
    .to_string();

    assert!(expanded.contains("expected `name = ResolverType`"), "{expanded}");
}
