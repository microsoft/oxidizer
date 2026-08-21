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
pub(crate) fn expand(input: DeriveInput) -> TokenStream {
    let mut errors = Errors::default();

    let expanded = parse::parse(input, &mut errors)
        .and_then(|ast| validate::validate(ast, &mut errors))
        .filter(|_| errors.is_empty())
        .map(|model| generate::generate(&model));

    expanded.unwrap_or_else(|| errors.into_compile_error())
}

#[cfg(test)]
mod tests {
    use syn::parse_quote;

    use super::*;

    /// Expands the derive and pretty-prints the result.
    fn rendered(input: DeriveInput) -> String {
        let expanded = expand(input);
        let file: syn::File = syn::parse2(expanded).expect("the expansion parses as a file");
        prettyplease::unparse(&file)
    }

    #[test]
    fn a_valid_input_expands_to_every_item() {
        insta::assert_snapshot!(rendered(parse_quote! {
            #[display("failed for {path}")]
            #[from(std::io::Error)]
            struct T { path: String, inner: ohno::OhnoCore }
        }));
    }

    #[test]
    fn the_suppressing_flags_remove_their_items() {
        insta::assert_snapshot!(rendered(parse_quote! {
            #[no_debug]
            #[no_constructors]
            struct T { inner: ohno::OhnoCore }
        }));
    }

    #[test]
    fn a_rejected_input_expands_to_diagnostics_only() {
        insta::assert_snapshot!(rendered(parse_quote!(
            enum T {
                A,
            }
        )));
    }

    #[test]
    fn a_valid_shape_with_an_invalid_template_generates_nothing() {
        // Generation is all-or-nothing: a fault anywhere means no items are emitted, so `rustc`
        // reports the fault rather than the fault plus every use of a type that never appeared.
        insta::assert_snapshot!(rendered(parse_quote! {
            #[display("bad path: {pth}")]
            struct T { path: String, inner: ohno::OhnoCore }
        }));
    }
}
