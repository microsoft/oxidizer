// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use quote::quote;
use syn::{Data, DeriveInput, Fields, Result};

use crate::derive_error::types::{BacktracePolicy, ErrorFieldRef, FromConfig};
use crate::utils::bail;

/// Generate From trait implementations for specified types
pub fn generate_from_implementations(
    input: &DeriveInput,
    error_field: &ErrorFieldRef,
    from_configs: &[FromConfig],
    backtrace_policy: BacktracePolicy,
) -> Result<proc_macro2::TokenStream> {
    if from_configs.is_empty() {
        return Ok(quote! {});
    }

    let Data::Struct(data_struct) = &input.data else {
        bail!("From implementations only support structs");
    };

    match &data_struct.fields {
        Fields::Named(_) => generate_from_implementations_named(input, error_field, from_configs, backtrace_policy),
        Fields::Unnamed(_) => generate_from_implementations_tuple(input, error_field, from_configs, backtrace_policy),
        Fields::Unit => bail!("From implementations not supported for unit structs"),
    }
}

#[expect(clippy::option_if_let_else, reason = "Would decrease readability")]
fn generate_from_implementations_named(
    input: &DeriveInput,
    error_field: &ErrorFieldRef,
    from_configs: &[FromConfig],
    backtrace_policy: BacktracePolicy,
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

    let caused_by_core = backtrace_policy.to_builder_call_with_error();

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
                        #error_field_access: #caused_by_core,
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
    backtrace_policy: BacktracePolicy,
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
    let caused_by_core = backtrace_policy.to_builder_call_with_error();

    let from_impls = from_configs.iter().map(|from_config| {
        let from_type = &from_config.from_type;

        // Generate default values for all fields except the error field
        // Note: Field expressions for tuple structs would use indices like "0", "1", etc.
        let field_defaults: Vec<proc_macro2::TokenStream> = (0..total_fields)
            .map(|i| {
                if i == error_field_index {
                    caused_by_core.clone()
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

    use syn::{parse_quote, parse_str};

    use super::*;
    use crate::utils::assert_formatted_snapshot;

    // Helper: common FromConfig for std::io::Error without field expressions
    fn cfg_io() -> FromConfig {
        FromConfig {
            from_type: parse_str("std::io::Error").unwrap(),
            field_expressions: HashMap::new(),
        }
    }

    #[test]
    fn no_configs_produces_empty_output() {
        let input: DeriveInput = parse_quote! {
            struct MyError {
                source: ohno::OhnoCore,
            }
        };
        let error_field_ref = ErrorFieldRef::Named(syn::Ident::new("source", proc_macro2::Span::call_site()));
        let result = generate_from_implementations(&input, &error_field_ref, &[], BacktracePolicy::Auto).unwrap();
        assert!(result.is_empty(), "Expected empty token stream when no from_configs are provided");
    }

    #[test]
    fn named_struct_with_generics() {
        let input: DeriveInput = parse_quote! {
            struct MyError<T: std::fmt::Debug> {
                source: ohno::OhnoCore,
                other: T,
            }
        };
        let error_field_ref = ErrorFieldRef::Named(syn::Ident::new("source", proc_macro2::Span::call_site()));
        let result = generate_from_implementations(&input, &error_field_ref, &[cfg_io()], BacktracePolicy::Auto).unwrap();
        assert_formatted_snapshot!(result);
    }

    #[test]
    fn tuple_struct_default_fields() {
        let input: DeriveInput = parse_quote! {
            struct SimpleTupleError(ohno::OhnoCore, String);
        };
        let error_field_ref = ErrorFieldRef::Indexed(syn::Index::from(0));
        let result = generate_from_implementations(&input, &error_field_ref, &[cfg_io()], BacktracePolicy::Auto).unwrap();
        assert_formatted_snapshot!(result);
    }

    // Consolidated: all non-struct inputs to tuple implementation helper produce identical bail message.
    #[test]
    fn tuple_non_struct_variants_bail() {
        let cases: &[DeriveInput] = &[
            // enum (basic)
            parse_quote! { enum MyError { A, B } },
            // union
            parse_quote! { union MyUnion { a: u32 } },
            // generic enum
            parse_quote! { enum GenericEnumError<T> { A(T) } },
        ];
        for input in cases {
            let error_field_ref = ErrorFieldRef::Indexed(syn::Index::from(0));
            let err = generate_from_implementations_tuple(input, &error_field_ref, &[cfg_io()], BacktracePolicy::Auto).unwrap_err();
            assert_eq!(err.to_string(), "From implementations for tuple structs only support structs");
        }
    }

    #[test]
    fn tuple_impl_rejects_named_fields() {
        let input: DeriveInput = parse_quote! {
            struct MyError {
                source: ohno::OhnoCore,
                message: String,
            }
        };
        let error_field_ref = ErrorFieldRef::Indexed(syn::Index::from(0));
        let err = generate_from_implementations_tuple(&input, &error_field_ref, &[cfg_io()], BacktracePolicy::Auto).unwrap_err();
        assert_eq!(
            err.to_string(),
            "From implementations for tuple structs only support structs with unnamed fields"
        );
    }

    #[test]
    fn tuple_struct_error_field_at_different_positions() {
        let input: DeriveInput = parse_quote! {
            struct TupleError(String, ohno::OhnoCore, i32);
        };
        let error_field_ref = ErrorFieldRef::Indexed(syn::Index::from(1));
        let result = generate_from_implementations(&input, &error_field_ref, &[cfg_io()], BacktracePolicy::Auto).unwrap();
        assert_formatted_snapshot!(result);
    }

    #[test]
    fn tuple_struct_with_custom_field_expressions() {
        let input: DeriveInput = parse_quote! {
            struct TupleError(ohno::OhnoCore, String, i32);
        };
        let error_field_ref = ErrorFieldRef::Indexed(syn::Index::from(0));
        let mut field_expressions = HashMap::new();
        field_expressions.insert("1".to_string(), parse_str("String::from(\"custom\")").unwrap());
        field_expressions.insert("2".to_string(), parse_str("42").unwrap());
        let cfg = FromConfig {
            from_type: parse_str("std::io::Error").unwrap(),
            field_expressions,
        };
        let result = generate_from_implementations(&input, &error_field_ref, &[cfg], BacktracePolicy::Auto).unwrap();
        assert_formatted_snapshot!(result);
    }

    #[test]
    fn non_struct_rejected_at_top_level() {
        let input: DeriveInput = parse_quote! {
            enum NotAStruct { A, B }
        };
        let error_field_ref = ErrorFieldRef::Named(syn::Ident::new("dummy", proc_macro2::Span::call_site()));
        let err = generate_from_implementations(&input, &error_field_ref, &[cfg_io()], BacktracePolicy::Auto).unwrap_err();
        assert_eq!(err.to_string(), "From implementations only support structs");
    }

    #[test]
    fn named_struct_with_custom_field_expression() {
        let input: DeriveInput = parse_quote! {
            struct NamedErr {
                source: ohno::OhnoCore,
                detail: String,
            }
        };
        let error_field_ref = ErrorFieldRef::Named(syn::Ident::new("source", proc_macro2::Span::call_site()));
        let mut field_expressions = HashMap::new();
        field_expressions.insert("detail".to_string(), parse_str("String::from(\"custom\")").unwrap());
        let cfg = FromConfig {
            from_type: parse_str("std::io::Error").unwrap(),
            field_expressions,
        };
        let tokens = generate_from_implementations_named(&input, &error_field_ref, &[cfg], BacktracePolicy::Auto).unwrap();
        assert_formatted_snapshot!(tokens);
    }

    #[test]
    fn named_struct_default_fields() {
        let input: DeriveInput = parse_quote! {
            struct SimpleNamed {
                source: ohno::OhnoCore,
                info: String,
            }
        };
        let error_field_ref = ErrorFieldRef::Named(syn::Ident::new("source", proc_macro2::Span::call_site()));
        let tokens = generate_from_implementations_named(&input, &error_field_ref, &[cfg_io()], BacktracePolicy::Auto).unwrap();
        assert_formatted_snapshot!(tokens);
    }

    #[test]
    fn named_impl_rejects_indexed_error_field() {
        let input: DeriveInput = parse_quote! {
            struct NamedStruct {
                source: ohno::OhnoCore,
                message: String,
            }
        };
        let error_field_ref = ErrorFieldRef::Indexed(syn::Index::from(0));
        let err = generate_from_implementations_named(&input, &error_field_ref, &[cfg_io()], BacktracePolicy::Auto).unwrap_err();
        assert_eq!(err.to_string(), "Expected named field for named struct");
    }

    #[test]
    fn named_impl_rejects_non_struct() {
        let input: DeriveInput = parse_quote! {
            enum NotStruct { A }
        };
        let error_field_ref = ErrorFieldRef::Named(syn::Ident::new("source", proc_macro2::Span::call_site()));
        let err = generate_from_implementations_named(&input, &error_field_ref, &[cfg_io()], BacktracePolicy::Auto).unwrap_err();
        assert_eq!(err.to_string(), "From implementations for named structs only support structs");
    }

    #[test]
    fn named_impl_requires_named_fields() {
        let input: DeriveInput = parse_quote! {
            struct TupleStructError(ohno::OhnoCore, String);
        };
        let error_field_ref = ErrorFieldRef::Named(syn::Ident::new("source", proc_macro2::Span::call_site()));
        let err = generate_from_implementations_named(&input, &error_field_ref, &[cfg_io()], BacktracePolicy::Auto).unwrap_err();
        assert_eq!(
            err.to_string(),
            "From implementations for named structs only support structs with named fields"
        );
    }

    #[test]
    fn unit_struct_rejected() {
        let input: DeriveInput = parse_quote! {
            struct UnitStructError;
        };
        let error_field_ref = ErrorFieldRef::Named(syn::Ident::new("source", proc_macro2::Span::call_site()));
        let err = generate_from_implementations(&input, &error_field_ref, &[cfg_io()], BacktracePolicy::Auto).unwrap_err();
        assert_eq!(err.to_string(), "From implementations not supported for unit structs");
    }

    #[test]
    fn tuple_impl_rejects_named_error_field() {
        let input: DeriveInput = parse_quote! {
            struct MismatchTuple(ohno::OhnoCore, String);
        };
        let error_field_ref = ErrorFieldRef::Named(syn::Ident::new("source", proc_macro2::Span::call_site()));
        let err = generate_from_implementations_tuple(&input, &error_field_ref, &[cfg_io()], BacktracePolicy::Auto).unwrap_err();
        assert_eq!(err.to_string(), "Expected indexed field for tuple struct");
    }
}
