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
use super::model::{Model, ModelField, Position, Shape};

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

/// Builds a `Self { .. }` or `Self(..)` literal.
///
/// `core` initializes the field holding the core. `data` is called once per other field, in
/// declaration order, and is given the field together with its position among the non-core ones —
/// the order in which a caller that laid its values out in advance holds them.
///
/// Taking the core's initializer apart from the rest is what keeps a generator from having to find
/// the core again: [`Shape`] already knows which field it is, so no caller compares members and
/// none carries an index of its own that could drift out of step with the field list.
#[must_use]
pub(crate) fn construct(shape: &Shape, core: &TokenStream, mut data: impl FnMut(&ModelField, usize) -> TokenStream) -> TokenStream {
    // One walk, so the members and their values cannot be paired by two iterators that only happen
    // to agree.
    let initialized = shape.positions().map(|(field, position)| {
        let value = match position {
            Position::Core => core.clone(),
            Position::Data(index) => data(field, index),
        };

        (&field.member, value)
    });

    match shape.style {
        Style::Named => {
            let assignments = initialized.map(|(member, value)| quote!(#member: #value));
            quote!(Self { #(#assignments,)* })
        }
        Style::Tuple => {
            let values = initialized.map(|(_, value)| value);
            quote!(Self(#(#values,)*))
        }
    }
}

/// The member of the field holding the core, ready to quote as `self.#member`.
#[must_use]
pub(crate) fn core_member(model: &Model) -> &Member {
    &model.shape.core().member
}
