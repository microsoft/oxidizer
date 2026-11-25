// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! In-process privacy macro input validation.

use data_privacy_macros_impl::classified::classified;
use data_privacy_macros_impl::derive::{redacted_debug, redacted_display};
use data_privacy_macros_impl::taxonomy::taxonomy;
use insta::{assert_snapshot, assert_yaml_snapshot};
use quote::quote;
use testing_aids::render_expansion;

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
    .unwrap();
    assert_snapshot!(render_expansion(&output));
}

#[test]
fn gamma_survivor_invalid_rust_is_not_default_input() {
    let mut diagnostics = Vec::new();
    for input in [quote!(), quote!(enum), quote!(struct)] {
        for (name, result) in [
            ("classified", classified(quote!(Privacy::Secret), input.clone())),
            ("redacted_debug", redacted_debug(input.clone())),
            ("redacted_display", redacted_display(input.clone())),
        ] {
            diagnostics.push((name, input.to_string(), result.unwrap_err().to_string()));
        }
    }
    for args in [quote!(123), quote!(Privacy::)] {
        let error = classified(
            args.clone(),
            quote!(
                struct Example(u32);
            ),
        )
        .unwrap_err();
        diagnostics.push(("classified attribute", args.to_string(), error.to_string()));
    }
    assert_yaml_snapshot!(diagnostics);
}
