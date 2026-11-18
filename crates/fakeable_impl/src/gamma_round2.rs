// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use quote::quote;
use syn::parse_quote;

use super::*;

#[test]
fn gamma_round2_non_cfg_attribute_does_not_apply_nested_configuration() {
    let attr = parse_quote!(#[custom(any(), cfg(any()))]);
    assert!(!cfg_attr_applies_attribute(&attr, "cfg"));
}

#[test]
fn gamma_round2_qualified_single_segment_impl_is_rejected() {
    let item = parse_quote!(
        impl<T> ::Service {}
    );
    assert_eq!(
        extract_struct_segment(&item).unwrap_err().to_string(),
        "qualified impl targets are not supported; apply the impl in the service's declaration module"
    );
}

#[test]
fn gamma_round2_constructor_is_not_a_receiver_returning_self() {
    let info = extract_method_info(&parse_quote!(fn create() -> Self)).unwrap();
    assert!(info.is_constructor);
    assert!(!info.returns_self_with_receiver);
    let info = extract_method_info(&parse_quote!(fn update(&self) -> Self)).unwrap();
    assert!(!info.is_constructor);
    assert!(info.returns_self_with_receiver);
}

#[test]
fn gamma_round2_lifetime_is_inserted_before_all_type_parameters() {
    let result = add_explicit_lifetimes(&parse_quote!(fn apply<T, U, const N: usize>(value: Wrapper<&str>)));
    assert_eq!(
        result.to_token_stream().to_string(),
        quote!(fn apply<'mock, T, U, const N: usize>(value: Wrapper<&'mock str>)).to_string()
    );
}

#[test]
fn gamma_round2_cfg_attr_ignores_nonmatching_sibling_attributes() {
    assert!(cfg_attr_tokens_apply_attribute(quote!(feature = "x", inline, must_use), "must_use"));
    assert!(!cfg_attr_tokens_apply_attribute(quote!(feature = "x"), "must_use"));
}

#[test]
fn gamma_round2_expect_condition_is_not_an_attribute() {
    let tokens = quote!(expect(dead_code), expect(unused), cfg_attr(any(), expect(unused_variables)));
    assert_eq!(
        rewrite_expect_in_tokens(tokens).to_string(),
        quote!(expect(dead_code), allow(unused), cfg_attr(any(), allow(unused_variables))).to_string()
    );
}

#[test]
fn gamma_round2_ordinary_attribute_arguments_are_not_rewritten() {
    let mut meta = parse_quote!(custom(any(), expect(dead_code)));
    rewrite_expect_meta(&mut meta);
    assert_eq!(
        meta.to_token_stream().to_string(),
        quote!(custom(any(), expect(dead_code))).to_string()
    );
}

#[cfg(feature = "mockall")]
#[test]
fn gamma_round2_nested_fn_elision_and_explicit_trait_binders() {
    assert!(type_contains_implicit_higher_ranked_elision(&parse_quote!(
        for<'a> fn(fn(&str), &'a str)
    )));
    assert!(!type_contains_implicit_higher_ranked_elision(&parse_quote!(
        dyn for<'a> Fn(&'a str, &str)
    )));
}
