// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

mod debug;
mod display;
mod tuple;

use syn::parse_quote;

use super::*;
use crate::utils::assert_formatted_snapshot;

#[test]
fn test_basic_error_struct_expansion() {
    let input: DeriveInput = parse_quote! {
        #[derive(Error)]
        struct TestError {
            message: String,
            #[error]
            inner: OhnoCore,
        }
    };

    let result = impl_error_derive(&input).unwrap();
    assert_formatted_snapshot!(result);
}

#[test]
fn test_from_attribute_expansion() {
    let input: DeriveInput = parse_quote! {
        #[derive(Error)]
        #[from(std::io::Error)]
        struct IoWrapperError {
            #[error]
            inner: OhnoCore,
        }
    };

    let result = impl_error_derive(&input).unwrap();
    assert_formatted_snapshot!(result);
}

#[test]
fn test_no_debug_attribute_expansion() {
    let input: DeriveInput = parse_quote! {
        #[derive(Error)]
        #[no_debug]
        struct NoDebugError {
            message: String,
            #[error]
            inner: OhnoCore,
        }
    };

    let result = impl_error_derive(&input).unwrap();
    assert_formatted_snapshot!(result);
}

#[test]
fn test_no_constructors_expansion() {
    let input: DeriveInput = parse_quote! {
        #[derive(Error)]
        #[no_constructors]
        struct NoConstructorsError {
            message: String,
            #[error]
            inner: OhnoCore,
        }
    };

    let result = impl_error_derive(&input).unwrap();
    assert_formatted_snapshot!(result);
}

#[test]
fn test_generate_from_implementations_tuple() {
    // Test that generate_from_implementations_tuple doesn't return Ok(Default::default())
    let input: DeriveInput = parse_quote! {
        #[derive(Error)]
        #[from(String)]
        struct TestError(String, #[error] OhnoCore);
    };

    let result = crate::derive_error::impl_error_derive(&input).unwrap();
    assert_formatted_snapshot!(result);
}

#[test]
fn test_ohno_core_first_position_struct() {
    let input: DeriveInput = parse_quote! {
        #[derive(Error)]
        struct FirstPositionError {
            #[error]
            inner: OhnoCore,
            message: String,
            code: u32,
        }
    };

    let result = impl_error_derive(&input).unwrap();
    assert_formatted_snapshot!(result);
}

#[test]
fn test_ohno_core_middle_position_struct() {
    let input: DeriveInput = parse_quote! {
        #[derive(Error)]
        struct MiddlePositionError {
            message: String,
            #[error]
            inner: OhnoCore,
            code: u32,
        }
    };

    let result = impl_error_derive(&input).unwrap();
    assert_formatted_snapshot!(result);
}

