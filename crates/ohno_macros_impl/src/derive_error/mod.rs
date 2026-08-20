// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! `#[derive(ohno::Error)]`.
//!
//! Three phases, in order, each with one job:
//!
//! ```text
//! TokenStream --parse--> Ast --validate--> Model --generate--> TokenStream
//!               syntax          rules              rendering
//! ```

pub(crate) mod ast;
pub(crate) mod display;
pub(crate) mod generate;
pub(crate) mod model;
pub(crate) mod parse;
pub(crate) mod validate;

use proc_macro2::TokenStream;
use syn::DeriveInput;

use crate::diagnostics::Errors;

/// Expands the derive, or renders everything that stopped it.
#[must_use]
pub(crate) fn expand(input: DeriveInput) -> TokenStream {
    let mut errors = Errors::default();

    let expanded = parse::parse(input, &mut errors)
        .and_then(|ast| validate::validate(ast, &mut errors))
        .filter(|_| errors.is_empty())
        .map(|model| generate::generate(&model));

    expanded.unwrap_or_else(|| errors.into_compile_error())
}
