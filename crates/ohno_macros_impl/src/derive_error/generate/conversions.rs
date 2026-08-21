// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The generated conversions: one `From<T>` per `#[from(...)]` entry, and `From<Infallible>`.

use proc_macro2::TokenStream;
use quote::quote;

use super::construct;
use crate::derive_error::model::Model;
use crate::paths;

/// Every conversion the derive owes.
#[must_use]
pub(crate) fn generate(model: &Model) -> TokenStream {
    let from_types = model.conversions.iter().map(|conversion| from_type(model, conversion));
    let infallible = from_infallible(model);

    quote! {
        #(#from_types)*
        #infallible
    }
}

/// One `From<T>`, building the core from the source error and every other field from its
/// initializer.
///
/// The data initializers are evaluated into a single tuple first, because they may borrow `error`
/// while the core consumes it, and a struct literal evaluates its fields in the order they are
/// written. One tuple rather than one local per field, so that no generated name is in scope while
/// a later initializer is evaluated: an initializer naming an outer item cannot be captured by a
/// binding the derive introduced.
fn from_type(model: &Model, conversion: &crate::derive_error::model::Conversion) -> TokenStream {
    let (impl_generics, ty_generics, where_clause) = model.generics.split_for_impl();
    let ident = &model.ident;
    let source = &conversion.source;
    let core_path = paths::ohno_core();
    let core_member = &model.shape.core().member;
    let values = conversion.initializers();

    let mut data = (0..values.len()).map(syn::Index::from);
    let initializers: Vec<TokenStream> = model
        .shape
        .all()
        .map(|field| {
            if field.member == *core_member {
                quote!(#core_path::from(error))
            } else {
                let index = data.next().expect("one tuple element per non-core field");
                quote!(__ohno_fields.#index)
            }
        })
        .collect();

    let body = construct(&model.shape, &initializers);

    quote! {
        #[automatically_derived]
        impl #impl_generics ::core::convert::From<#source> for #ident #ty_generics #where_clause {
            fn from(error: #source) -> Self {
                let __ohno_fields = (#(#values,)*);
                #body
            }
        }
    }
}

/// `From<Infallible>`, generated for every error type.
///
/// A fallible conversion in user code can then be written once and stay correct when the error type
/// it converts from becomes infallible.
fn from_infallible(model: &Model) -> TokenStream {
    let (impl_generics, ty_generics, where_clause) = model.generics.split_for_impl();
    let ident = &model.ident;

    quote! {
        #[automatically_derived]
        impl #impl_generics ::core::convert::From<::core::convert::Infallible> for #ident #ty_generics #where_clause {
            fn from(_value: ::core::convert::Infallible) -> Self {
                ::core::unreachable!("Infallible cannot be constructed")
            }
        }
    }
}
