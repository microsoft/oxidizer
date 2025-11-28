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
fn redacted_debug_multiple() {
    let input = quote! {
        struct EmailAddress(String, String);
    };

    let result = data_privacy_macros_impl::derive::redacted_debug(input);
    let result_file = syn::parse_file(&result.unwrap().to_string()).unwrap();
    let pretty = prettyplease::unparse(&result_file);

    assert_snapshot!(pretty);
}

#[test]
fn redacted_debug_multiple_named() {
    let input = quote! {
        struct EmailAddress {
            x: String,
            y: String
        }
    };

    let result = data_privacy_macros_impl::derive::redacted_debug(input);
    let result_file = syn::parse_file(&result.unwrap().to_string()).unwrap();
    let pretty = prettyplease::unparse(&result_file);

    assert_snapshot!(pretty);
}

#[test]
fn redacted_debug_unit() {
    let input = quote! {
        struct EmailAddress;
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
fn redacted_display_multiple() {
    let input = quote! {
        struct EmailAddress(String, String);
    };

    let result = data_privacy_macros_impl::derive::redacted_display(input);
    let result_file = syn::parse_file(&result.unwrap().to_string()).unwrap();
    let pretty = prettyplease::unparse(&result_file);

    assert_snapshot!(pretty);
}

#[test]
fn redacted_display_multiple_named() {
    let input = quote! {
        struct EmailAddress {
            x: String,
            y: String
        }
    };

    let result = data_privacy_macros_impl::derive::redacted_display(input);
    let result_file = syn::parse_file(&result.unwrap().to_string()).unwrap();
    let pretty = prettyplease::unparse(&result_file);

    assert_snapshot!(pretty);
}

#[test]
fn redacted_display_unit() {
    let input = quote! {
        struct EmailAddress;
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

#[test]
fn redacted_to_string_multiple() {
    let input = quote! {
        struct EmailAddress(String, String);
    };

    let result = data_privacy_macros_impl::derive::redacted_to_string(input);
    let result_file = syn::parse_file(&result.unwrap().to_string()).unwrap();
    let pretty = prettyplease::unparse(&result_file);

    assert_snapshot!(pretty);
}

#[test]
fn redacted_to_string_multiple_named() {
    let input = quote! {
        struct EmailAddress {
            x: String,
            y: String
        }
    };

    let result = data_privacy_macros_impl::derive::redacted_to_string(input);
    let result_file = syn::parse_file(&result.unwrap().to_string()).unwrap();
    let pretty = prettyplease::unparse(&result_file);

    assert_snapshot!(pretty);
}

#[test]
fn redacted_to_string_unit() {
    let input = quote! {
        struct EmailAddress;
    };

    let result = data_privacy_macros_impl::derive::redacted_to_string(input);
    let result_file = syn::parse_file(&result.unwrap().to_string()).unwrap();
    let pretty = prettyplease::unparse(&result_file);

    assert_snapshot!(pretty);
}
