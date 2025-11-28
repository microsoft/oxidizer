// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use insta::assert_snapshot;
use quote::quote;

#[test]
fn redacted_debug() {
    let input = quote! {
        struct EmailAddress(String);
    };

    let result = data_privacy_macros_impl::derive::redacted_debug(input);
    let result_file = syn::parse_file(&result.unwrap().to_string()).unwrap();
    let pretty = prettyplease::unparse(&result_file);

    assert_snapshot!(pretty);
}

#[test]
fn redacted_display() {
    let input = quote! {
        struct EmailAddress(String);
    };

    let result = data_privacy_macros_impl::derive::redacted_display(input);
    let result_file = syn::parse_file(&result.unwrap().to_string()).unwrap();
    let pretty = prettyplease::unparse(&result_file);

    assert_snapshot!(pretty);
}

#[test]
fn redacted_to_string() {
    let input = quote! {
        struct EmailAddress(String);
    };

    let result = data_privacy_macros_impl::derive::redacted_to_string(input);
    let result_file = syn::parse_file(&result.unwrap().to_string()).unwrap();
    let pretty = prettyplease::unparse(&result_file);

    assert_snapshot!(pretty);
}
