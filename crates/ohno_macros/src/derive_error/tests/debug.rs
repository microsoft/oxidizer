// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use pretty_assertions::assert_eq;
use syn::{DeriveInput, parse_quote};

use crate::derive_error::generate_debug_impl;

#[test]
fn test_generate_debug_impl_unit_struct() {
    // Test line 210: syn::Fields::Unit case
    let input: DeriveInput = parse_quote! {
        struct UnitStruct;
    };

    let name = &input.ident;
    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();

    let result = generate_debug_impl(&input, name, &impl_generics, &ty_generics, where_clause);

    // Parse and format the result for comparison
    let result_string = result.to_string();

    // Verify it generates the unit struct debug implementation
    // The generated code uses ":: core :: fmt" with spaces
    assert!(result_string.contains("debug_struct"));
    assert!(result_string.contains("UnitStruct"));
    assert!(result_string.contains("finish"));

    // More precise check: parse as an impl and verify structure
    let impl_block: syn::ItemImpl = syn::parse2(result).expect("Should parse as valid impl");

    assert_eq!(impl_block.trait_.as_ref().unwrap().1.segments.last().unwrap().ident, "Debug");
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

    let result_string = result.to_string();

    // Verify it generates the enum debug implementation with finish_non_exhaustive
    assert!(result_string.contains("debug_struct"));
    assert!(result_string.contains("TestEnum"));
    assert!(result_string.contains("finish_non_exhaustive"));

    // Verify it's a valid impl block
    let impl_block: syn::ItemImpl = syn::parse2(result).expect("Should parse as valid impl");
    assert_eq!(impl_block.trait_.as_ref().unwrap().1.segments.last().unwrap().ident, "Debug");
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

    let result_string = result.to_string();

    // Verify it generates the named struct debug implementation
    assert!(result_string.contains("debug_struct"));
    assert!(result_string.contains("NamedStruct"));
    assert!(result_string.contains(r#""field1""#));
    assert!(result_string.contains(r#""field2""#));
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

    let result_string = result.to_string();

    // Verify it generates the tuple struct debug implementation
    assert!(result_string.contains("debug_tuple"));
    assert!(result_string.contains("TupleStruct"));
    assert!(result_string.contains("self . 0"));
    assert!(result_string.contains("self . 1"));
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

    let result_string = result.to_string();

    // Verify generics are included - with spaces in token stream
    assert!(result_string.contains("< T >"));
    assert!(result_string.contains("GenericStruct"));
}
