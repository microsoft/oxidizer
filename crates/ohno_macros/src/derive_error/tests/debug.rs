// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use syn::{DeriveInput, parse_quote};

use crate::derive_error::generate_debug_impl;
use crate::utils::assert_formatted_snapshot;

#[test]
fn test_generate_debug_impl_unit_struct() {
    // Test line 210: syn::Fields::Unit case
    let input: DeriveInput = parse_quote! {
        struct UnitStruct;
    };

    let name = &input.ident;
    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();

    let result = generate_debug_impl(&input, name, &impl_generics, &ty_generics, where_clause);
    assert_formatted_snapshot!(result);
}

#[test]
fn test_generate_debug_impl_enum() {
    // Test line 215: _ (enum) case
    let input: DeriveInput = parse_quote! {
        enum TestEnum {
            Variant1,
            Variant2(String),
        }
    };

    let name = &input.ident;
    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();

    let result = generate_debug_impl(&input, name, &impl_generics, &ty_generics, where_clause);
    assert_formatted_snapshot!(result);
}

#[test]
fn test_generate_debug_impl_named_fields() {
    // Test named fields case for completeness
    let input: DeriveInput = parse_quote! {
        struct NamedStruct {
            field1: String,
            field2: i32,
        }
    };

    let name = &input.ident;
    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();

    let result = generate_debug_impl(&input, name, &impl_generics, &ty_generics, where_clause);
    assert_formatted_snapshot!(result);
}

#[test]
fn test_generate_debug_impl_tuple_struct() {
    // Test tuple struct case for completeness
    let input: DeriveInput = parse_quote! {
        struct TupleStruct(String, i32);
    };

    let name = &input.ident;
    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();

    let result = generate_debug_impl(&input, name, &impl_generics, &ty_generics, where_clause);
    assert_formatted_snapshot!(result);
}

#[test]
fn test_generate_debug_impl_with_generics() {
    // Test that generics are properly handled
    let input: DeriveInput = parse_quote! {
        struct GenericStruct<T> {
            value: T,
        }
    };

    let name = &input.ident;
    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();

    let result = generate_debug_impl(&input, name, &impl_generics, &ty_generics, where_clause);
    assert_formatted_snapshot!(result);
}
