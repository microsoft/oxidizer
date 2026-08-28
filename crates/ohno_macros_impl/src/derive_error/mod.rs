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
use syn::{DeriveInput, Member};

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

/// Renders a member the way a diagnostic spells it, and the way `Debug` labels it: `path`, or `0`.
///
/// Shared by all three phases: a member reaches the user as text in a diagnostic, in a template
/// resolution and in a `Debug` label, and the three have to agree on how it is spelled.
#[must_use]
pub(crate) fn member_name(member: &Member) -> String {
    match member {
        Member::Named(ident) => ident.to_string(),
        Member::Unnamed(index) => index.index.to_string(),
    }
}
