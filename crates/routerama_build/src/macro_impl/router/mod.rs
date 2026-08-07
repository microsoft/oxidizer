// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Implementation of `#[router]`.
//!
//! A `#[router]` impl is lowered into three cooperating pieces:
//!
//! - a private route `enum` expanded by the existing `#[resolver]` generator,
//!   which supplies the compiled static trie, the configured dynamic builder,
//!   and typed path captures;
//! - a private response body sum with one variant per response-producing site,
//!   so heterogeneous handler, rejection, catcher, and interceptor bodies share
//!   one unboxed concrete type; and
//! - the generated entry points, which resolve, evaluate request predicates,
//!   run interceptors, extract, and call the selected handler directly.

mod emit;
mod model;
mod parse;

use alloc::vec::Vec;

use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::{ImplItem, ItemImpl};

use self::model::RouterArgs;
use super::resolver::{crate_path, route_runtime_path};

/// Expands `#[router]`.
#[must_use]
pub(crate) fn expand(attr: TokenStream2, item: TokenStream2) -> TokenStream2 {
    syn::parse2::<RouterArgs>(attr)
        .and_then(|args| syn::parse2::<ItemImpl>(item).map(|item| (args, item)))
        .and_then(|(args, item)| expand_router(args, item))
        .unwrap_or_else(syn::Error::into_compile_error)
}

fn expand_router(args: RouterArgs, mut item: ItemImpl) -> syn::Result<TokenStream2> {
    let rt = route_runtime_path();
    let root = crate_path();
    let router = parse::parse_router(args, &item)?;
    let groups = emit::build_groups(&router)?;
    let generated = emit::emit(&router, &groups, &rt, &root)?;

    strip_helper_attributes(&mut item);
    item.items.extend(generated.impl_items);
    let items = generated.items;

    Ok(quote! {
        #items
        #item
    })
}

/// Removes the inert helper attributes the macro consumed.
fn strip_helper_attributes(item: &mut ItemImpl) {
    const METHOD_ATTRS: [&str; 6] = ["route", "before", "after", "transform", "fallback", "catch"];
    const PARAM_ATTRS: [&str; 2] = ["body", "capture"];

    for impl_item in &mut item.items {
        let ImplItem::Fn(method) = impl_item else {
            continue;
        };
        method
            .attrs
            .retain(|attribute| !METHOD_ATTRS.iter().any(|name| attribute.path().is_ident(name)));
        for input in &mut method.sig.inputs {
            if let syn::FnArg::Typed(input) = input {
                input
                    .attrs
                    .retain(|attribute| !PARAM_ATTRS.iter().any(|name| attribute.path().is_ident(name)));
            }
        }
    }
    let _unused: Vec<()> = Vec::new();
}
