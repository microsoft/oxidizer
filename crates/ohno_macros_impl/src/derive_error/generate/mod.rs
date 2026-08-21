// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! [`Model`] into tokens.
//!
//! This phase returns a `TokenStream`, not a `Result`. It cannot fail, because a `Model` that would
//! make it fail cannot be built. That is what keeps a diagnostic out of generated code: a generator
//! that could fail would have to either thread a `Result` up or emit tokens `rustc` rejects at a
//! span the user never wrote.

pub(crate) mod constructors;
pub(crate) mod conversions;
pub(crate) mod traits;

use proc_macro2::TokenStream;
use quote::quote;
use syn::Member;

use super::ast::Style;
use super::model::{Model, Shape};

/// Generates every item the derive owes for `model`.
#[must_use]
pub(crate) fn generate(model: &Model) -> TokenStream {
    let display = traits::display(model);
    let error = traits::error(model);
    let enrichable = traits::enrichable(model);
    let error_ext = traits::error_ext(model);
    let debug = traits::debug(model);
    let constructors = constructors::generate(model);
    let conversions = conversions::generate(model);

    quote! {
        #display
        #error
        #enrichable
        #error_ext
        #debug
        #constructors
        #conversions
    }
}

/// Builds a `Self { .. }` or `Self(..)` literal from one initializer per field.
///
/// The initializers arrive in declaration order, so they line up with [`Shape::all`].
#[must_use]
pub(crate) fn construct(shape: &Shape, initializers: &[TokenStream]) -> TokenStream {
    match shape.style {
        Style::Named => {
            let assignments = shape.all().zip(initializers).map(|(field, value)| {
                let member = &field.member;
                quote!(#member: #value)
            });
            quote!(Self { #(#assignments,)* })
        }
        Style::Tuple => quote!(Self(#(#initializers,)*)),
    }
}

/// The member of the field holding the core, ready to quote as `self.#member`.
#[must_use]
pub(crate) fn core_member(model: &Model) -> &Member {
    &model.shape.core().member
}
