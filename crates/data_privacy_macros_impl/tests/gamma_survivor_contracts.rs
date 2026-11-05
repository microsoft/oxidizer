// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! In-process privacy macro input validation.

use data_privacy_macros_impl::classified::classified;
use data_privacy_macros_impl::derive::{redacted_debug, redacted_display};
use data_privacy_macros_impl::taxonomy::taxonomy;
use quote::quote;

#[test]
fn gamma_survivor_taxonomy_snake_case_has_no_leading_underscore() {
    let output = taxonomy(
        quote!(privacy),
        quote!(
            enum Privacy {
                UserName,
                IPAddress,
            }
        ),
    )
    .unwrap()
    .to_string();
    assert!(output.contains("\"user_name\""));
    assert!(output.contains("classify_user_name"));
    assert!(!output.contains("\"_user_name\""));
    assert!(output.contains("\"i_p_address\""));
}

#[test]
fn gamma_survivor_invalid_rust_is_not_default_input() {
    for input in [quote!(), quote!(enum), quote!(struct)] {
        classified(quote!(Privacy::Secret), input.clone()).unwrap_err();
        redacted_debug(input.clone()).unwrap_err();
        redacted_display(input).unwrap_err();
    }
    classified(
        quote!(123),
        quote!(
            struct Example(u32);
        ),
    )
    .unwrap_err();
    classified(
        quote!(Privacy::),
        quote!(
            struct Example(u32);
        ),
    )
    .unwrap_err();
}
