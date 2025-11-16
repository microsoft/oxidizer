// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use proc_macro2::Span;
use quote::quote;
use syn::{Data, DeriveInput, Fields, Result};

use crate::derive_error::types::ErrorFieldRef;
use crate::utils::bail;

/// Generate constructor methods for the error struct
pub fn generate_constructor_methods(input: &DeriveInput, error_field: &ErrorFieldRef) -> Result<proc_macro2::TokenStream> {
    let Data::Struct(data_struct) = &input.data else {
        bail!("Constructor generation only supports structs");
    };

    match &data_struct.fields {
        Fields::Named(_) => generate_constructor_methods_named(input, error_field),
        Fields::Unnamed(_) => generate_constructor_methods_tuple(input, error_field),
        Fields::Unit => bail!("Constructor generation not supported for unit structs"),
    }
}

fn generate_constructor_methods_named(input: &DeriveInput, error_field: &ErrorFieldRef) -> Result<proc_macro2::TokenStream> {
    let name = &input.ident;
    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();

    let ErrorFieldRef::Named(error_field_name) = error_field else {
        bail!("Expected named field for named struct");
    };

    let Data::Struct(data_struct) = &input.data else {
        bail!("Constructor generation for named structs only support structs");
    };

    let Fields::Named(fields) = &data_struct.fields else {
        bail!("Constructor generation for named structs only support structs with named fields");
    };

    // Get all fields except the error field
    let non_error_fields: Vec<_> = fields
        .named
        .iter()
        .filter(|field| field.ident.as_ref().is_some_and(|ident| ident != error_field_name))
        .collect();

    let new_method = syn::Ident::new("new", Span::call_site());
    let caused_by_method = syn::Ident::new("caused_by", Span::call_site());
    let error_field_access = error_field.to_field_access();

    if non_error_fields.is_empty() {
        // Simple case: only error field
        Ok(generate_simple_named_constructors(
            name,
            &impl_generics,
            &ty_generics,
            where_clause,
            &new_method,
            &caused_by_method,
            &error_field_access,
        ))
    } else {
        // Complex case: multiple fields
        Ok(generate_complex_named_constructors(
            name,
            &impl_generics,
            &ty_generics,
            where_clause,
            &new_method,
            &caused_by_method,
            &error_field_access,
            &non_error_fields,
        ))
    }
}

fn generate_constructor_methods_tuple(input: &DeriveInput, error_field: &ErrorFieldRef) -> Result<proc_macro2::TokenStream> {
    let name = &input.ident;
    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();

    let ErrorFieldRef::Indexed(index) = error_field else {
        bail!("Expected indexed field for tuple struct");
    };
    let error_field_index = index.index as usize;

    let Data::Struct(data_struct) = &input.data else {
        bail!("Constructor generation for tuple structs only support structs");
    };

    let Fields::Unnamed(fields) = &data_struct.fields else {
        bail!("Constructor generation for tuple structs only support structs with unnamed fields");
    };

    let total_fields = fields.unnamed.len();
    let new_method = syn::Ident::new("new", Span::call_site());
    let caused_by_method = syn::Ident::new("caused_by", Span::call_site());

    if total_fields == 1 {
        // Simple case: only error field
        Ok(generate_simple_tuple_constructors(
            name,
            &impl_generics,
            &ty_generics,
            where_clause,
            &new_method,
            &caused_by_method,
        ))
    } else {
        // Complex case: multiple fields
        Ok(generate_complex_tuple_constructors(
            name,
            &impl_generics,
            &ty_generics,
            where_clause,
            &new_method,
            &caused_by_method,
            error_field_index,
            fields,
        ))
    }
}

// Helper functions for generating constructor implementations

fn generate_simple_named_constructors(
    name: &syn::Ident,
    impl_generics: &syn::ImplGenerics,
    ty_generics: &syn::TypeGenerics,
    where_clause: Option<&syn::WhereClause>,
    new_method: &syn::Ident,
    caused_by_method: &syn::Ident,
    error_field_access: &proc_macro2::TokenStream,
) -> proc_macro2::TokenStream {
    quote! {
        impl #impl_generics #name #ty_generics #where_clause {
            /// Creates a new error with default message.
            pub(crate) fn #new_method() -> Self {
                Self {
                    #error_field_access: ohno::OhnoCore::default(),
                }
            }

            /// Creates a new error with a specified error.
            pub(crate) fn #caused_by_method(error: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> Self {
                Self {
                    #error_field_access: ohno::OhnoCore::from(error),
                }
            }
        }
    }
}

#[expect(clippy::too_many_arguments, reason = "C'est la vie")]
fn generate_complex_named_constructors(
    name: &syn::Ident,
    impl_generics: &syn::ImplGenerics,
    ty_generics: &syn::TypeGenerics,
    where_clause: Option<&syn::WhereClause>,
    new_method: &syn::Ident,
    caused_by_method: &syn::Ident,
    error_field_access: &proc_macro2::TokenStream,
    non_error_fields: &[&syn::Field],
) -> proc_macro2::TokenStream {
    let field_names: Vec<_> = non_error_fields.iter().map(|field| field.ident.as_ref().unwrap()).collect();
    let field_types: Vec<_> = non_error_fields.iter().map(|field| &field.ty).collect();

    // Use field names as parameter names to match the expected API
    let param_names = &field_names;

    quote! {
        impl #impl_generics #name #ty_generics #where_clause {
            /// Creates a new error with custom fields and default message.
            pub(crate) fn #new_method(#(#param_names: impl Into<#field_types>),*) -> Self {
                Self {
                    #(#field_names: #param_names.into(),)*
                    #error_field_access: ohno::OhnoCore::default(),
                }
            }

            /// Creates a new error with custom fields and a specified error.
            pub(crate) fn #caused_by_method(#(#param_names: impl Into<#field_types>,)* error: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> Self {
                Self {
                    #(#field_names: #param_names.into(),)*
                    #error_field_access: ohno::OhnoCore::from(error),
                }
            }
        }
    }
}

fn generate_simple_tuple_constructors(
    name: &syn::Ident,
    impl_generics: &syn::ImplGenerics,
    ty_generics: &syn::TypeGenerics,
    where_clause: Option<&syn::WhereClause>,
    new_method: &syn::Ident,
    caused_by_method: &syn::Ident,
) -> proc_macro2::TokenStream {
    quote! {
        impl #impl_generics #name #ty_generics #where_clause {
            /// Creates a new error with default message.
            pub(crate) fn #new_method() -> Self {
                Self(ohno::OhnoCore::default())
            }

            /// Creates a new error with a specified error.
            pub(crate) fn #caused_by_method(error: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> Self {
                Self(ohno::OhnoCore::from(error))
            }
        }
    }
}

#[expect(clippy::too_many_arguments, reason = "C'est la vie")]
fn generate_complex_tuple_constructors(
    name: &syn::Ident,
    impl_generics: &syn::ImplGenerics,
    ty_generics: &syn::TypeGenerics,
    where_clause: Option<&syn::WhereClause>,
    new_method: &syn::Ident,
    caused_by_method: &syn::Ident,
    error_field_index: usize,
    fields: &syn::FieldsUnnamed,
) -> proc_macro2::TokenStream {
    // Get all field types except the error field
    let mut field_types = Vec::new();
    let mut param_names = Vec::new();
    let mut field_assignments = Vec::new();

    for (i, field) in fields.unnamed.iter().enumerate() {
        if i == error_field_index {
            field_assignments.push(quote! { ohno::OhnoCore::default() });
        } else {
            field_types.push(&field.ty);
            let param_name = syn::Ident::new(&format!("param_{i}"), Span::call_site());
            param_names.push(param_name.clone());
            field_assignments.push(quote! { #param_name.into() });
        }
    }

    // For caused_by method, we need to handle the error field differently
    let mut caused_by_assignments = Vec::new();
    let mut caused_by_param_idx = 0;
    for (i, _) in fields.unnamed.iter().enumerate() {
        if i == error_field_index {
            caused_by_assignments.push(quote! { ohno::OhnoCore::from(error) });
        } else {
            let param_name = &param_names[caused_by_param_idx];
            caused_by_assignments.push(quote! { #param_name.into() });
            caused_by_param_idx += 1;
        }
    }

    quote! {
        impl #impl_generics #name #ty_generics #where_clause {
            /// Creates a new error with custom fields and default message.
            pub(crate) fn #new_method(#(#param_names: impl Into<#field_types>),*) -> Self {
                Self(#(#field_assignments),*)
            }

            /// Creates a new error with custom fields and a specified error.
            pub(crate) fn #caused_by_method(#(#param_names: impl Into<#field_types>,)* error: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> Self {
                Self(#(#caused_by_assignments),*)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use syn::{Index, parse_quote};

    fn named_field(name: &str) -> ErrorFieldRef {
        ErrorFieldRef::Named(syn::Ident::new(name, Span::call_site()))
    }

    fn indexed_field(index: usize) -> ErrorFieldRef {
        ErrorFieldRef::Indexed(Index::from(index))
    }

    fn expect_message(result: syn::Result<proc_macro2::TokenStream>, expected: &str) {
        let err = result.expect_err("expected constructor generation to fail");
        assert_eq!(err.to_string(), expected);
    }

    #[test]
    fn generate_constructors_rejects_non_struct_inputs() {
        let input: DeriveInput = parse_quote! {
            enum NotStruct {
                Variant,
            }
        };

        expect_message(
            generate_constructor_methods(&input, &named_field("inner")),
            "Constructor generation only supports structs",
        );
    }

    #[test]
    fn generate_constructors_rejects_unit_structs() {
        let input: DeriveInput = parse_quote! {
            struct UnitError;
        };

        expect_message(
            generate_constructor_methods(&input, &named_field("inner")),
            "Constructor generation not supported for unit structs",
        );
    }

    #[test]
    fn named_constructors_require_named_error_refs() {
        let input: DeriveInput = parse_quote! {
            struct SampleError {
                #[error]
                inner: OhnoCore,
            }
        };

        expect_message(
            generate_constructor_methods_named(&input, &indexed_field(0)),
            "Expected named field for named struct",
        );
    }

    #[test]
    fn named_constructors_require_struct_input() {
        let input: DeriveInput = parse_quote! {
            enum NotStruct {
                Variant,
            }
        };

        expect_message(
            generate_constructor_methods_named(&input, &named_field("inner")),
            "Constructor generation for named structs only support structs",
        );
    }

    #[test]
    fn named_constructors_require_named_fields() {
        let input: DeriveInput = parse_quote! {
            struct TupleLike(#[error] OhnoCore);
        };

        expect_message(
            generate_constructor_methods_named(&input, &named_field("inner")),
            "Constructor generation for named structs only support structs with named fields",
        );
    }

    #[test]
    fn tuple_constructors_require_indexed_error_refs() {
        let input: DeriveInput = parse_quote! {
            struct TupleError(#[error] OhnoCore);
        };

        expect_message(
            generate_constructor_methods_tuple(&input, &named_field("inner")),
            "Expected indexed field for tuple struct",
        );
    }

    #[test]
    fn tuple_constructors_require_struct_input() {
        let input: DeriveInput = parse_quote! {
            enum NotStruct {
                Variant,
            }
        };

        expect_message(
            generate_constructor_methods_tuple(&input, &indexed_field(0)),
            "Constructor generation for tuple structs only support structs",
        );
    }

    #[test]
    fn tuple_constructors_require_unnamed_fields() {
        let input: DeriveInput = parse_quote! {
            struct NamedFields {
                #[error]
                inner: OhnoCore,
            }
        };

        expect_message(
            generate_constructor_methods_tuple(&input, &indexed_field(0)),
            "Constructor generation for tuple structs only support structs with unnamed fields",
        );
    }
}
