// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use data_privacy_macros_impl::derive::{redacted_debug, redacted_display, redacted_to_string};
use insta::assert_snapshot;
use quote::quote;

macro_rules! test_derive {
    ($input:expr, $derive_fn:path) => {{
        let result = $derive_fn($input);
        let result_file = syn::parse_file(&result.unwrap_or_else(|e| e.to_compile_error()).to_string()).unwrap();
        let pretty = prettyplease::unparse(&result_file);
        assert_snapshot!(pretty);
    }};
}

#[test]
fn redacted_debug_single() {
    let input = quote! {
        struct EmailAddress(String);
    };

    test_derive!(input, redacted_debug);
}

#[test]
fn redacted_debug_multiple() {
    let input = quote! {
        struct EmailAddress(String, String);
    };

    test_derive!(input, redacted_debug);
}

#[test]
fn redacted_debug_multiple_named() {
    let input = quote! {
        struct EmailAddress {
            x: String,
            y: String
        }
    };

    test_derive!(input, redacted_debug);
}

#[test]
fn redacted_debug_unit() {
    let input = quote! {
        struct EmailAddress;
    };

    test_derive!(input, redacted_debug);
}

#[test]
fn redacted_debug_enum() {
    let input = quote! {
        enum EmailAddress {
            A
        }
    };

    test_derive!(input, redacted_debug);
}


#[test]
fn redacted_display_single() {
    let input = quote! {
        struct EmailAddress(String);
    };

    test_derive!(input, redacted_display);
}

#[test]
fn redacted_display_multiple() {
    let input = quote! {
        struct EmailAddress(String, String);
    };

    test_derive!(input, redacted_display);
}

#[test]
fn redacted_display_multiple_named() {
    let input = quote! {
        struct EmailAddress {
            x: String,
            y: String
        }
    };

    test_derive!(input, redacted_display);
}

#[test]
fn redacted_display_unit() {
    let input = quote! {
        struct EmailAddress;
    };

    test_derive!(input, redacted_display);
}

#[test]
fn redacted_display_enum() {
    let input = quote! {
        enum EmailAddress {
            A
        }
    };

    test_derive!(input, redacted_display);
}

