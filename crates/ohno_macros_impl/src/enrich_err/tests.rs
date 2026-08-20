// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

// miri fails to use insta snapshots: `insta::_macro_support::get_cargo_workspace` leads to
// `unsupported operation: `open` not available when isolation is enabled`
use quote::quote;
use syn::parse_quote;

use super::*;

/// Expands the attribute and pretty-prints the result.
///
/// The whole expansion is snapshotted rather than searched for substrings, because what the
/// wrapper has to get right is the shape of the code it emits: which body runs where, what the
/// message is built from, and which parts of the signature survive. A substring assertion can
/// only confirm that some token is present somewhere, so it passes on an expansion that is
/// wrong everywhere else.
fn rendered(args: TokenStream, item: Item) -> String {
    let expanded = expand(args, item);
    let file: syn::File = syn::parse2(expanded).expect("the expansion parses as a file");
    prettyplease::unparse(&file)
}

#[test]
fn a_bare_attribute_names_the_function() {
    insta::assert_snapshot!(rendered(
        TokenStream::new(),
        parse_quote! {
            fn load() -> Result<(), MyError> { Err(MyError::new()) }
        }
    ));
}

#[test]
fn a_literal_message_renders_without_format() {
    insta::assert_snapshot!(rendered(
        quote!("operation failed"),
        parse_quote!(
            fn load() -> Result<(), MyError> {
                todo!()
            }
        )
    ));
}

#[test]
fn an_inline_capture_goes_through_format() {
    insta::assert_snapshot!(rendered(
        quote!("failed for {path}"),
        parse_quote!(
            fn load(path: &str) -> Result<(), MyError> {
                todo!()
            }
        )
    ));
}

#[test]
fn arguments_are_passed_through_unchanged() {
    insta::assert_snapshot!(rendered(
        quote!("read {} bytes", data.len()),
        parse_quote!(
            fn load(data: &[u8]) -> Result<(), MyError> {
                todo!()
            }
        )
    ));
}

#[test]
fn a_self_prefixed_argument_is_left_alone() {
    insta::assert_snapshot!(rendered(
        quote!("counter {}", self.counter),
        parse_quote!(
            fn load(&self) -> Result<(), MyError> {
                todo!()
            }
        )
    ));
}

#[test]
fn the_body_runs_inside_a_closure() {
    insta::assert_snapshot!(rendered(
        quote!("failed"),
        parse_quote!(
            fn load() -> Result<(), MyError> {
                Err(MyError::new())
            }
        )
    ));
}

#[test]
fn an_async_function_awaits_an_async_block() {
    insta::assert_snapshot!(rendered(
        quote!("failed"),
        parse_quote!(
            async fn load() -> Result<(), MyError> {
                Err(MyError::new())
            }
        )
    ));
}

#[test]
fn the_signature_survives_untouched() {
    insta::assert_snapshot!(rendered(
        quote!("failed"),
        parse_quote! {
            /// Documented.
            pub unsafe extern "C" fn load<T: Clone>(&mut self, value: T) -> Result<T, MyError>
            where
                T: Send,
            { todo!() }
        }
    ));
}

#[test]
fn a_missing_return_type_is_rejected() {
    insta::assert_snapshot!(rendered(
        quote!("failed"),
        parse_quote!(
            fn load() {}
        )
    ));
}

#[test]
fn a_non_literal_first_argument_is_rejected() {
    insta::assert_snapshot!(rendered(
        quote!(not_a_literal),
        parse_quote!(
            fn load() -> Result<(), MyError> {
                todo!()
            }
        )
    ));
}

#[test]
fn a_non_function_is_rejected() {
    insta::assert_snapshot!(rendered(
        quote!("failed"),
        parse_quote!(
            struct T;
        )
    ));
}
