// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! In-process bundle type-state contracts.

use fundle_macros_impl::bundle;
use quote::quote;

#[test]
fn gamma_round2_as_ref_requires_only_the_selected_field_set() {
    let output = bundle(
        quote!(),
        quote!(
            struct Example {
                a: u32,
                b: String,
                c: bool,
            }
        ),
    )
    .unwrap();
    syn::parse2::<syn::File>(output.clone()).unwrap();
    let text = output.to_string();
    for expected in [
        quote!(AsRef<u32> for ExampleBuilder<RW, ::fundle::Set, B, C>),
        quote!(AsRef<String> for ExampleBuilder<RW, A, ::fundle::Set, C>),
        quote!(AsRef<bool> for ExampleBuilder<RW, A, B, ::fundle::Set>),
    ] {
        assert!(text.contains(&expected.to_string()), "{expected}");
    }
}

#[test]
fn gamma_survivor_builder_and_selection_preserve_other_type_states() {
    let output = bundle(
        quote!(),
        quote!(
            struct Example {
                a: u32,
                b: String,
            }
        ),
    )
    .unwrap();
    syn::parse2::<syn::File>(output.clone()).unwrap();
    let text = output.to_string();
    for expected in [
        quote!(ExampleBuilder<RW, ::fundle::Set, T2>),
        quote!(ExampleBuilder<RW, T1, ::fundle::Set>),
        quote!(Select<'a, RW, ::fundle::Set, T2>),
        quote!(Select<'a, RW, T1, ::fundle::Set>),
    ] {
        assert!(text.contains(&expected.to_string()), "{expected}");
    }
}

#[test]
fn gamma_survivor_bundle_rejects_malformed_struct_input() {
    for input in [
        quote!(),
        quote!(struct),
        quote!(
            enum Example {}
        ),
    ] {
        bundle(quote!(), input).unwrap_err();
    }
}
