// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The generated constructors, `new` and `caused_by`.
//!
//! Both are `pub(crate)`, whatever the visibility of the error type. They are an implementation
//! convenience for the crate that owns the error, not part of its public API, so adding a field is
//! not a breaking change for callers. An error type that needs a public constructor declares one by
//! hand, under `#[no_constructors]`.

use proc_macro2::TokenStream;
use quote::quote;

use super::construct;
use crate::derive_error::model::Model;
use crate::paths;

/// The constructors, unless `#[no_constructors]` was written.
#[must_use]
pub(crate) fn generate(model: &Model) -> TokenStream {
    if !model.constructors {
        return TokenStream::new();
    }

    let (impl_generics, ty_generics, where_clause) = model.generics.split_for_impl();
    let ident = &model.ident;
    let core = paths::ohno_core();

    let parameters = model.shape.data().map(|field| {
        let binding = &field.binding;
        let ty = &field.ty;
        quote!(#binding: impl ::core::convert::Into<#ty>)
    });
    let parameters = parameters.collect::<Vec<_>>();

    let new_body = construct(&model.shape, &initializers(model, &quote!(#core::default())));
    let caused_by_body = construct(&model.shape, &initializers(model, &quote!(#core::from(error))));

    quote! {
        impl #impl_generics #ident #ty_generics #where_clause {
            /// Creates the error with no source.
            #[allow(dead_code, reason = "generated for every error type, used at the author's discretion")]
            pub(crate) fn new(#(#parameters),*) -> Self {
                #new_body
            }

            /// Creates the error wrapping `error` as its source.
            #[allow(dead_code, reason = "generated for every error type, used at the author's discretion")]
            pub(crate) fn caused_by(
                #(#parameters,)*
                error: impl ::core::convert::Into<::std::boxed::Box<dyn ::std::error::Error + ::core::marker::Send + ::core::marker::Sync>>,
            ) -> Self {
                #caused_by_body
            }
        }
    }
}

/// One initializer per field in declaration order, with `core` used for the error field.
fn initializers(model: &Model, core: &TokenStream) -> Vec<TokenStream> {
    let core_member = &model.shape.core().member;

    model
        .shape
        .all()
        .map(|field| {
            if field.member == *core_member {
                core.clone()
            } else {
                let binding = &field.binding;
                quote!(#binding.into())
            }
        })
        .collect()
}
