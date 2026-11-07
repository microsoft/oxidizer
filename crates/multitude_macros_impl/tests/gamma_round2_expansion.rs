// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Error recovery and helper-name uniqueness for multiple custom variants.

use multitude_macros_impl::derive_deserialize_in;
use quote::quote;

#[test]
fn gamma_round2_missing_attribute_values_are_diagnostics_not_panics() {
    for source in [
        quote!(
            #[multitude(crate)]
            struct Record {
                value: u8,
            }
        ),
        quote!(
            #[serde(expecting)]
            struct Record {
                value: u8,
            }
        ),
        quote!(
            struct Record {
                #[multitude(deserialize_with)]
                value: u8,
            }
        ),
        quote!(
            struct Record {
                #[serde(alias)]
                value: u8,
            }
        ),
        quote!(
            struct Record {
                #[serde(deserialize_with)]
                value: u8,
            }
        ),
        quote!(
            #[serde(transparent)]
            struct Record {
                #[serde(alias)]
                value: u8,
            }
        ),
        quote!(
            enum Record {
                #[serde(deserialize_with)]
                One(u8),
            }
        ),
    ] {
        let output = derive_deserialize_in(source, &syn::parse_quote!(::multitude::de));
        assert!(output.to_string().contains("compile_error"), "{output}");
        assert!(output.to_string().contains("expected `=`"), "{output}");
    }
}

#[test]
fn gamma_round2_custom_variant_seeds_are_unique_for_nonzero_indices() {
    let output = derive_deserialize_in(
        quote! {
            enum Record {
                Plain,
                #[serde(deserialize_with = "decode_first")] First(u32),
                #[serde(deserialize_with = "decode_second")] Second { value: u32 },
            }
        },
        &syn::parse_quote!(::multitude::de),
    );
    syn::parse2::<syn::File>(output.clone()).unwrap();
    let output = output.to_string();
    assert!(output.contains("struct __MultitudeVisitorForRecordVariant1Seed"));
    assert!(output.contains("struct __MultitudeVisitorForRecordVariant2Seed"));
    assert!(!output.contains("struct __MultitudeVisitorForRecordVariant0Seed"));
    assert!(output.contains("decode_first (deserializer)"));
    assert!(output.contains("decode_second (deserializer)"));
}

#[test]
fn gamma_round2_generated_deserializer_metadata_includes_aliases_and_excludes_skips() {
    let output = derive_deserialize_in(
        quote! {
            #[serde(deny_unknown_fields)]
            struct Record {
                #[serde(alias = "old")] value: u32,
                #[serde(skip)] hidden: u32,
            }
        },
        &syn::parse_quote!(::multitude::de),
    )
    .to_string();
    assert!(output.contains(&quote!(&["value", "old"]).to_string()), "{output}");
    assert!(
        output.contains(&quote!("value" => ::core::result::Result::Ok).to_string()),
        "{output}"
    );
    assert!(output.contains("unknown_field"));
    let output = derive_deserialize_in(
        quote!(
            enum Record {
                #[serde(alias = "old")]
                One,
                #[serde(skip)]
                Hidden,
                Two {
                    #[serde(alias = "legacy")]
                    value: u32,
                },
            }
        ),
        &syn::parse_quote!(::multitude::de),
    )
    .to_string();
    assert!(output.contains(&quote!(&["One", "old", "Two"]).to_string()), "{output}");
    assert!(output.contains(&quote!(&["value", "legacy"]).to_string()), "{output}");
    assert!(
        output.contains(&quote!("One" => ::core::result::Result::Ok).to_string()),
        "{output}"
    );
}
