// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use quote::quote;
use syn::{Data, DeriveInput, Fields, Result};

use crate::derive_error::types::{ErrorFieldRef, FromConfig};
use crate::utils::bail;

/// Generate From trait implementations for specified types
pub fn generate_from_implementations(
    input: &DeriveInput,
    error_field: &ErrorFieldRef,
    from_configs: &[FromConfig],
) -> Result<proc_macro2::TokenStream> {
    if from_configs.is_empty() {
        return Ok(quote! {});
    }

    let Data::Struct(data_struct) = &input.data else {
        bail!("From implementations only support structs");
    };

    match &data_struct.fields {
        Fields::Named(_) => generate_from_implementations_named(input, error_field, from_configs),
        Fields::Unnamed(_) => generate_from_implementations_tuple(input, error_field, from_configs),
        Fields::Unit => bail!("From implementations not supported for unit structs"),
    }
}

#[expect(clippy::option_if_let_else, reason = "Would decrease readability")]
fn generate_from_implementations_named(
    input: &DeriveInput,
    error_field: &ErrorFieldRef,
    from_configs: &[FromConfig],
) -> Result<proc_macro2::TokenStream> {
    let name = &input.ident;
    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();

    let ErrorFieldRef::Named(error_field_name) = error_field else {
        bail!("Expected named field for named struct");
    };

    let Data::Struct(data_struct) = &input.data else {
        bail!("From implementations for named structs only support structs");
    };

    let Fields::Named(fields) = &data_struct.fields else {
        bail!("From implementations for named structs only support structs with named fields");
    };

    // Get all field names except the error field
    let other_fields: Vec<_> = fields
        .named
        .iter()
        .filter_map(|field| field.ident.as_ref().filter(|ident| *ident != error_field_name))
        .collect();

    let from_impls = from_configs.iter().map(|from_config| {
        let from_type = &from_config.from_type;

        // Generate field initializations
        let field_defaults = other_fields.iter().map(|field_name| {
            let field_name_str = field_name.to_string();
            if let Some(custom_expr) = from_config.field_expressions.get(&field_name_str) {
                // Use the custom expression provided in the attribute
                quote! { #field_name: #custom_expr }
            } else {
                // Default to Default::default() for fields not specified
                quote! { #field_name: Default::default() }
            }
        });

        let error_field_access = error_field.to_field_access();

        quote! {
            impl #impl_generics From<#from_type> for #name #ty_generics #where_clause {
                fn from(error: #from_type) -> Self {
                    Self {
                        #(#field_defaults,)*
                        #error_field_access: ohno::OhnoCore::from(error),
                    }
                }
            }
        }
    });

    Ok(quote! {
        #(#from_impls)*
    })
}

#[expect(clippy::option_if_let_else, reason = "Would decrease readability")]
fn generate_from_implementations_tuple(
    input: &DeriveInput,
    error_field: &ErrorFieldRef,
    from_configs: &[FromConfig],
) -> Result<proc_macro2::TokenStream> {
    let name = &input.ident;
    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();

    let ErrorFieldRef::Indexed(index) = error_field else {
        bail!("Expected indexed field for tuple struct");
    };
    let error_field_index = index.index as usize;

    let Data::Struct(data_struct) = &input.data else {
        bail!("From implementations for tuple structs only support structs");
    };

    let Fields::Unnamed(fields) = &data_struct.fields else {
        bail!("From implementations for tuple structs only support structs with unnamed fields");
    };

    let total_fields = fields.unnamed.len();

    let from_impls = from_configs.iter().map(|from_config| {
        let from_type = &from_config.from_type;

        // Generate default values for all fields except the error field
        // Note: Field expressions for tuple structs would use indices like "0", "1", etc.
        let field_defaults: Vec<proc_macro2::TokenStream> = (0..total_fields)
            .map(|i| {
                if i == error_field_index {
                    quote! { ohno::OhnoCore::from(error) }
                } else {
                    let field_index_str = i.to_string();
                    if let Some(custom_expr) = from_config.field_expressions.get(&field_index_str) {
                        quote! { #custom_expr }
                    } else {
                        quote! { Default::default() }
                    }
                }
            })
            .collect();

        quote! {
            impl #impl_generics From<#from_type> for #name #ty_generics #where_clause {
                fn from(error: #from_type) -> Self {
                    Self(#(#field_defaults),*)
                }
            }
        }
    });

    Ok(quote! {
        #(#from_impls)*
    })
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use syn::parse_str;

    use super::*;

    // Helper: parse DeriveInput from snippet
    fn di(src: &str) -> DeriveInput {
        parse_str(src).unwrap()
    }
    // Helper: common FromConfig for std::io::Error without field expressions
    fn cfg_io() -> FromConfig {
        FromConfig {
            from_type: parse_str("std::io::Error").unwrap(),
            field_expressions: HashMap::new(),
        }
    }

    #[test]
    fn test_generate_from_implementations_no_configs() {
        let input = di(r"struct MyError { source: ohno::OhnoCore, }");
        let error_field_ref = ErrorFieldRef::Named(syn::Ident::new("source", proc_macro2::Span::call_site()));
        let result = generate_from_implementations(&input, &error_field_ref, &[]).unwrap();
        assert!(result.is_empty(), "Expected empty token stream when no from_configs are provided");
    }

    #[test]
    fn test_generate_from_implementations_named_with_generics() {
        let input = di(r"struct MyError<T: std::fmt::Debug> { source: ohno::OhnoCore, other: T, }");
        let error_field_ref = ErrorFieldRef::Named(syn::Ident::new("source", proc_macro2::Span::call_site()));
        let result = generate_from_implementations(&input, &error_field_ref, &[cfg_io()]).unwrap();
        let expected = quote! {
            impl<T: std::fmt::Debug> From<std::io::Error> for MyError<T> {
                fn from(error: std::io::Error) -> Self {
                    Self { other: Default::default(), source: ohno::OhnoCore::from(error), }
                }
            }
        };
        assert_eq!(result.to_string(), expected.to_string());
    }

    #[test]
    fn test_generate_from_implementations_tuple_default_fields() {
        let input = di(r"struct SimpleTupleError(ohno::OhnoCore, String);");
        let error_field_ref = ErrorFieldRef::Indexed(syn::Index::from(0));
        let result = generate_from_implementations(&input, &error_field_ref, &[cfg_io()]).unwrap();
        let expected = quote! {
            impl From<std::io::Error> for SimpleTupleError {
                fn from(error: std::io::Error) -> Self { Self(ohno::OhnoCore::from(error), Default::default()) }
            }
        };
        assert_eq!(result.to_string(), expected.to_string());
    }

    // Consolidated: all non-struct inputs to tuple implementation helper produce identical bail message.
    #[test]
    fn test_tuple_non_struct_variants_bail() {
        let cases = [
            // enum (basic)
            r"enum MyError { A, B }",
            // union
            r"union MyUnion { a: u32, }",
            // generic enum
            r"enum GenericEnumError<T> { A(T) }",
        ];
        for src in cases {
            let input = di(src);
            let error_field_ref = ErrorFieldRef::Indexed(syn::Index::from(0));
            let result = generate_from_implementations_tuple(&input, &error_field_ref, &[cfg_io()]);
            assert!(result.is_err(), "Expected error for non-struct input: {src}");
            assert_eq!(
                result.unwrap_err().to_string(),
                "From implementations for tuple structs only support structs"
            );
        }
    }

    #[test]
    fn test_generate_from_implementations_tuple_named_fields() {
        let input = di(r"struct MyError { source: ohno::OhnoCore, message: String, }");
        let error_field_ref = ErrorFieldRef::Indexed(syn::Index::from(0));
        let result = generate_from_implementations_tuple(&input, &error_field_ref, &[cfg_io()]);
        assert!(result.is_err(), "Expected error for named fields in tuple function");
        assert_eq!(
            result.unwrap_err().to_string(),
            "From implementations for tuple structs only support structs with unnamed fields"
        );
    }

    #[test]
    fn test_generate_from_implementations_tuple_error_field_at_different_positions() {
        let input = di(r"struct TupleError(String, ohno::OhnoCore, i32);");
        let error_field_ref = ErrorFieldRef::Indexed(syn::Index::from(1));
        let result = generate_from_implementations(&input, &error_field_ref, &[cfg_io()]).unwrap();
        let expected = quote! {
            impl From<std::io::Error> for TupleError {
                fn from(error: std::io::Error) -> Self { Self(Default::default(), ohno::OhnoCore::from(error), Default::default()) }
            }
        };
        assert_eq!(result.to_string(), expected.to_string());
    }

    #[test]
    fn test_generate_from_implementations_tuple_with_custom_field_expressions() {
        let input = di(r"struct TupleError(ohno::OhnoCore, String, i32);");
        let error_field_ref = ErrorFieldRef::Indexed(syn::Index::from(0));
        let mut field_expressions = HashMap::new();
        field_expressions.insert("1".to_string(), parse_str("String::from(\"custom\")").unwrap());
        field_expressions.insert("2".to_string(), parse_str("42").unwrap());
        let cfg = FromConfig {
            from_type: parse_str("std::io::Error").unwrap(),
            field_expressions,
        };
        let result = generate_from_implementations(&input, &error_field_ref, &[cfg]).unwrap();
        let expected = quote! {
            impl From<std::io::Error> for TupleError {
                fn from(error: std::io::Error) -> Self { Self(ohno::OhnoCore::from(error), String::from("custom"), 42) }
            }
        };
        assert_eq!(result.to_string(), expected.to_string());
    }

    #[test]
    fn test_generate_from_implementations_non_struct() {
        let input = di(r"enum NotAStruct { A, B }");
        let error_field_ref = ErrorFieldRef::Named(syn::Ident::new("dummy", proc_macro2::Span::call_site()));
        let result = generate_from_implementations(&input, &error_field_ref, &[cfg_io()]);
        assert!(result.is_err(), "Expected error for non-struct type at top-level function");
        assert_eq!(result.unwrap_err().to_string(), "From implementations only support structs");
    }

    #[test]
    fn test_generate_from_implementations_named_pattern_success() {
        let input = di(r"struct NamedErr { source: ohno::OhnoCore, detail: String, }");
        let error_field_ref = ErrorFieldRef::Named(syn::Ident::new("source", proc_macro2::Span::call_site()));
        let mut field_expressions = HashMap::new();
        field_expressions.insert("detail".to_string(), parse_str("String::from(\"custom\")").unwrap());
        let cfg = FromConfig {
            from_type: parse_str("std::io::Error").unwrap(),
            field_expressions,
        };
        let tokens = generate_from_implementations_named(&input, &error_field_ref, &[cfg]).unwrap();
        let expected = quote! {
            impl From<std::io::Error> for NamedErr {
                fn from(error: std::io::Error) -> Self { Self { detail: String::from("custom"), source: ohno::OhnoCore::from(error), } }
            }
        };
        assert_eq!(tokens.to_string(), expected.to_string());
    }

    #[test]
    fn test_generate_from_implementations_named_pattern_success_default() {
        let input = di(r"struct SimpleNamed { source: ohno::OhnoCore, info: String, }");
        let error_field_ref = ErrorFieldRef::Named(syn::Ident::new("source", proc_macro2::Span::call_site()));
        let tokens = generate_from_implementations_named(&input, &error_field_ref, &[cfg_io()]).unwrap();
        let expected = quote! {
            impl From<std::io::Error> for SimpleNamed {
                fn from(error: std::io::Error) -> Self { Self { info: Default::default(), source: ohno::OhnoCore::from(error), } }
            }
        };
        assert_eq!(tokens.to_string(), expected.to_string());
    }

    #[test]
    fn test_generate_from_implementations_named_error_field_mismatch() {
        let input = di(r"struct NamedStruct { source: ohno::OhnoCore, message: String, }");
        let error_field_ref = ErrorFieldRef::Indexed(syn::Index::from(0));
        let result = generate_from_implementations_named(&input, &error_field_ref, &[cfg_io()]);
        assert!(
            result.is_err(),
            "Expected error when passing Indexed field to named struct implementation"
        );
        assert_eq!(result.unwrap_err().to_string(), "Expected named field for named struct");
    }

    #[test]
    fn test_generate_from_implementations_named_non_struct() {
        let input = di(r"enum NotStruct { A }");
        let error_field_ref = ErrorFieldRef::Named(syn::Ident::new("source", proc_macro2::Span::call_site()));
        let result = generate_from_implementations_named(&input, &error_field_ref, &[cfg_io()]);
        assert!(result.is_err(), "Expected error for non-struct input inside named implementation");
        assert_eq!(
            result.unwrap_err().to_string(),
            "From implementations for named structs only support structs"
        );
    }

    #[test]
    fn test_generate_from_implementations_named_requires_named_fields() {
        let input = di(r"struct TupleStructError(ohno::OhnoCore, String);");
        let error_field_ref = ErrorFieldRef::Named(syn::Ident::new("source", proc_macro2::Span::call_site()));
        let result = generate_from_implementations_named(&input, &error_field_ref, &[cfg_io()]);
        assert!(
            result.is_err(),
            "Expected error for tuple struct passed to named implementation function"
        );
        assert_eq!(
            result.unwrap_err().to_string(),
            "From implementations for named structs only support structs with named fields"
        );
    }

    #[test]
    fn test_generate_from_implementations_unit_struct() {
        let input = di(r"struct UnitStructError;");
        let error_field_ref = ErrorFieldRef::Named(syn::Ident::new("source", proc_macro2::Span::call_site()));
        let result = generate_from_implementations(&input, &error_field_ref, &[cfg_io()]);
        assert!(result.is_err(), "Expected error for unit struct input at Fields::Unit arm");
        assert_eq!(
            result.unwrap_err().to_string(),
            "From implementations not supported for unit structs"
        );
    }

    #[test]
    fn test_generate_from_implementations_tuple_error_field_mismatch() {
        let input = di(r"struct MismatchTuple(ohno::OhnoCore, String);");
        let error_field_ref = ErrorFieldRef::Named(syn::Ident::new("source", proc_macro2::Span::call_site()));
        let result = generate_from_implementations_tuple(&input, &error_field_ref, &[cfg_io()]);
        assert!(
            result.is_err(),
            "Expected error for mismatch between tuple struct and named error field ref"
        );
        assert_eq!(result.unwrap_err().to_string(), "Expected indexed field for tuple struct");
    }
}
