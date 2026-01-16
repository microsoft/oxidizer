// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use syn::parse_quote;

use super::*;
use crate::utils::assert_formatted_snapshot;

#[test]
fn test_tuple_only_ohno_core() {
    let input: DeriveInput = parse_quote! {
        #[derive(Error)]
        struct OnlyCoreError(#[error] OhnoCore);
    };

    let result = impl_error_derive(&input).unwrap();
    assert_formatted_snapshot!(result);
}

#[test]
fn test_tuple_struct_expansion() {
    let input: DeriveInput = parse_quote! {
        #[derive(Error)]
        struct TupleError(String, #[error] OhnoCore);
    };

    let result = impl_error_derive(&input).unwrap();
    assert_formatted_snapshot!(result);
}

#[test]
fn test_ohno_core_first_position_tuple() {
    let input: DeriveInput = parse_quote! {
        #[derive(Error)]
        struct FirstPositionTupleError(#[error] OhnoCore, String, u32);
    };

    let result = impl_error_derive(&input).unwrap();
    assert_formatted_snapshot!(result);
}

#[test]
fn test_ohno_core_middle_position_tuple() {
    let input: DeriveInput = parse_quote! {
        #[derive(Error)]
        struct MiddlePositionTupleError(String, #[error] OhnoCore, u32);
    };

    let result = impl_error_derive(&input).unwrap();
    assert_formatted_snapshot!(result);
}
