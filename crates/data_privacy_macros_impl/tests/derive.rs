// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![expect(missing_docs, reason = "Test code")]

use data_privacy_macros_impl::derive::{redacted_debug, redacted_display};
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
#[cfg_attr(miri, ignore)]
fn redacted_debug_single() {
    let input = quote! {
        struct EmailAddress(String);
    };

    test_derive!(input, redacted_debug);
}

#[test]
#[cfg_attr(miri, ignore)]
fn redacted_debug_multiple() {
    let input = quote! {
        struct EmailAddress(String, #[unredacted] String);
    };

    test_derive!(input, redacted_debug);
}

#[test]
#[cfg_attr(miri, ignore)]
fn redacted_debug_multiple_named() {
    let input = quote! {
        struct EmailAddress {
            x: String,
            #[unredacted]
            y: String
        }
    };

    test_derive!(input, redacted_debug);
}

#[test]
#[cfg_attr(miri, ignore)]
fn redacted_debug_unit() {
    let input = quote! {
        struct EmailAddress;
    };

    test_derive!(input, redacted_debug);
}

#[test]
#[cfg_attr(miri, ignore)]
fn redacted_debug_enum() {
    let input = quote! {
        enum EmailAddress {
            A
        }
    };

    test_derive!(input, redacted_debug);
}

#[test]
#[cfg_attr(miri, ignore)]
fn redacted_display_single() {
    let input = quote! {
        struct EmailAddress(String);
    };

    test_derive!(input, redacted_display);
}

#[test]
#[cfg_attr(miri, ignore)]
fn redacted_display_multiple() {
    let input = quote! {
        struct EmailAddress(String, #[unredacted] String);
    };

    test_derive!(input, redacted_display);
}

#[test]
#[cfg_attr(miri, ignore)]
fn redacted_display_multiple_named() {
    let input = quote! {
        struct EmailAddress {
            x: String,
            #[unredacted]
            y: String
        }
    };

    test_derive!(input, redacted_display);
}

#[test]
#[cfg_attr(miri, ignore)]
fn redacted_display_unit() {
    let input = quote! {
        struct EmailAddress;
    };

    test_derive!(input, redacted_display);
}

#[test]
#[cfg_attr(miri, ignore)]
fn redacted_display_enum() {
    let input = quote! {
        enum EmailAddress {
            A
        }
    };

    test_derive!(input, redacted_display);
}
