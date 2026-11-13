// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use quote::{ToTokens, quote};
use syn::parse_quote;

use crate::field_attrs::{SharedFieldAttrs, extend_where_clause};

#[test]
fn gamma_round2_shared_attributes_report_consumption() {
    for attr in [
        parse_quote!(#[dimension(log = "renamed")]),
        parse_quote!(#[if_none("absent")]),
        parse_quote!(#[unredacted]),
        parse_quote!(#[data_class(SECRET)]),
    ] {
        assert!(SharedFieldAttrs::default().try_parse(&attr).unwrap());
    }
    assert!(!SharedFieldAttrs::default().try_parse(&parse_quote!(#[doc = "kept"])).unwrap());
}

#[test]
fn gamma_round2_empty_predicates_do_not_insert_where() {
    let mut generics = parse_quote!(<T>);
    extend_where_clause(&mut generics, []);
    assert_eq!(generics.to_token_stream().to_string(), quote!(<T>).to_string());
    assert!(generics.where_clause.is_none());
}
