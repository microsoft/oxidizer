// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Round-two event parser contracts.

use observed_macros_impl::{derive_enrichment, event};
use quote::quote;

#[test]
fn gamma_round2_optional_borrowed_enrichment_copies_the_pointee() {
    let output = derive_enrichment(quote!(
        struct Optional<'a> {
            #[unredacted]
            #[if_none(drop)]
            value: Option<&'a str>,
        }
    ))
    .unwrap();
    syn::parse2::<syn::File>(output.clone()).unwrap();
    assert!(
        output
            .to_string()
            .contains(&quote!(::std::sync::Arc::<str>::from(__val)).to_string())
    );
}

#[test]
fn gamma_round2_named_options_require_string_values() {
    for attr in [
        quote!(#[info(name = 42)]),
        quote!(#[counter(name = 42)]),
        quote!(#[counter(unit = 42)]),
        quote!(#[counter(desc = 42)]),
    ] {
        let error = event(quote!("sample"), quote!(#attr struct Sample {})).unwrap_err();
        assert_eq!(error.to_string(), "expected string literal");
    }
    let error = derive_enrichment(quote!(
        struct Sample {
            #[dimension(metric = 42)]
            value: u32,
        }
    ))
    .unwrap_err();
    assert_eq!(error.to_string(), "expected string literal");
}

#[test]
fn gamma_round2_bare_counter_has_no_explicit_description_or_unit() {
    let bare = event(
        quote!("sample"),
        quote!(
            #[counter]
            struct Sample {}
        ),
    )
    .unwrap();
    let explicit = event(
        quote!("sample"),
        quote!(
            #[counter()]
            struct Sample {}
        ),
    )
    .unwrap();
    assert_eq!(bare.to_string(), explicit.to_string());
}
