// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Shared helpers for the `#[router]` expansion tests.

#[cfg(feature = "tower")]
use alloc::string::{String, ToString as _};

use proc_macro2::TokenStream as TokenStream2;
#[cfg(feature = "tower")]
use quote::quote;
use syn::Type;

use super::{expand, expand_with_data};

pub(super) fn expand_router(item: TokenStream2) -> syn::Result<TokenStream2> {
    expand(
        syn::parse2(item).expect("test input is a syntactically valid impl"),
        None,
        false,
        false,
    )
}

pub(super) fn expand_fixed_router(state: Type, item: TokenStream2) -> syn::Result<TokenStream2> {
    expand(
        syn::parse2(item).expect("test input is a syntactically valid impl"),
        Some(state),
        false,
        false,
    )
}

pub(super) fn expand_heterogeneous_router(item: TokenStream2) -> syn::Result<TokenStream2> {
    expand_with_data(
        syn::parse2(item).expect("test input is a syntactically valid impl"),
        None,
        false,
        false,
        true,
    )
}

pub(super) fn expand_heterogeneous_generic_router(item: TokenStream2) -> syn::Result<TokenStream2> {
    expand_with_data(
        syn::parse2(item).expect("test input is a syntactically valid impl"),
        None,
        false,
        false,
        true,
    )
}

pub(super) fn expand_fixed_mounted_router(state: Type, item: TokenStream2) -> syn::Result<TokenStream2> {
    expand(
        syn::parse2(item).expect("test input is a syntactically valid impl"),
        Some(state),
        true,
        false,
    )
}

pub(super) fn expand_mounted_router(state: Type, item: TokenStream2) -> syn::Result<TokenStream2> {
    expand(
        syn::parse2(item).expect("test input is a syntactically valid impl"),
        Some(state),
        true,
        false,
    )
}

pub(super) fn expand_mounted_heterogeneous_router(state: Type, item: TokenStream2) -> syn::Result<TokenStream2> {
    expand_with_data(
        syn::parse2(item).expect("test input is a syntactically valid impl"),
        Some(state),
        true,
        false,
        true,
    )
}

#[cfg(feature = "tower")]
pub(super) fn expand_tower_router(state: Option<Type>, item: TokenStream2) -> syn::Result<TokenStream2> {
    expand(
        syn::parse2(item).expect("test input is a syntactically valid impl"),
        state,
        false,
        true,
    )
}

pub(super) fn has_route_method_attribute(file: &syn::File) -> bool {
    file.items.iter().any(|item| {
        let syn::Item::Impl(item) = item else {
            return false;
        };
        item.items.iter().any(|item| {
            let syn::ImplItem::Fn(method) = item else {
                return false;
            };
            method.attrs.iter().any(|attribute| attribute.path().is_ident("route"))
        })
    })
}

#[cfg(feature = "tower")]
pub(super) fn generated_method_signature(generated: &TokenStream2, name: &str) -> String {
    let file: syn::File = syn::parse2(generated.clone()).expect("the expansion is valid Rust");
    file.items
        .iter()
        .filter_map(|item| match item {
            syn::Item::Impl(item) => Some(item),
            _ => None,
        })
        .flat_map(|item| &item.items)
        .find_map(|item| match item {
            syn::ImplItem::Fn(method) if method.sig.ident == name => Some(quote! { #method }.to_string()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("the expansion contains a generated `{name}` method"))
}
