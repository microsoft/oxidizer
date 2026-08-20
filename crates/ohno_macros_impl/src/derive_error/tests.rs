// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

// miri fails to use insta snapshots: `insta::_macro_support::get_cargo_workspace` leads to
// `unsupported operation: `open` not available when isolation is enabled`
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
