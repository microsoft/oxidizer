// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use proc_macro::TokenStream;
use quote::quote;
use syn::{Data, DeriveInput, Fields, parse_macro_input, parse_quote};

use crate::utils::generate_unique_field_name;

/// Attribute macro version of `error_type` that can handle documentation comments.
///
/// Usage:
/// ```ignore
/// use ohno::error;
///
/// /// Documentation for the error type
/// #[error]
/// struct MyError;
/// ```
///
/// This macro converts a simple struct declaration into a complete error type
/// with `OhnoCore` integration, preserving any documentation comments.
///
/// It can also be applied to existing structs with fields, transforming them:
/// ```ignore
/// /// My awesome error
/// #[ohno::error]
/// #[derive(Debug)]
/// #[from(std::io::Error(kind: ErrorKind::Io))]
/// pub struct Error {
///     pub(crate) kind: ErrorKind,
/// }
/// ```
///
/// Into:
/// ```ignore
/// /// My awesome error
/// #[derive(Debug, ohno::Error)]
/// #[from(std::io::Error(kind: ErrorKind::Io))]
/// pub struct Error {
///     pub(crate) kind: ErrorKind,
///     ohno_core: OhnoCore,
/// }
/// ```
#[cfg_attr(test, mutants::skip)] // procedural macro API cannot be used in tests directly
pub fn error(_args: TokenStream, input: TokenStream) -> TokenStream {
    let mut input = parse_macro_input!(input as DeriveInput);

    add_fiasko_error_derive(&mut input);
    add_ohno_core_field(&mut input);

    TokenStream::from(quote! { #input })
}

fn add_fiasko_error_derive(input: &mut DeriveInput) {
    input.attrs.insert(
        0,
        parse_quote! {
            #[derive(ohno::Error)]
        },
    );
}

fn add_ohno_core_field(input: &mut DeriveInput) {
    if let Data::Struct(data_struct) = &mut input.data {
        match &mut data_struct.fields {
            Fields::Unit => {
                // Unit struct: convert to tuple struct with OhnoCore
                let field: syn::Field = parse_quote! {
                    #[error] ohno::OhnoCore
                };
                let mut fields = syn::punctuated::Punctuated::new();
                fields.push(field);
                data_struct.fields = Fields::Unnamed(syn::FieldsUnnamed {
                    paren_token: syn::token::Paren::default(),
                    unnamed: fields,
                });
            }
            Fields::Unnamed(fields) => {
                // Tuple struct: add OhnoCore as last field
                fields.unnamed.push(parse_quote! {
                    #[error] ohno::OhnoCore
                });
            }
            Fields::Named(fields) => {
                let names = fields
                    .named
                    .iter()
                    .map(|f| f.ident.as_ref().expect("unnamed field"))
                    .collect::<Vec<_>>();
                let field_name = generate_unique_field_name(&names);
                fields.named.push(parse_quote! {
                    #[error]
                    #field_name: ohno::OhnoCore
                });
            }
        }
    } else {
        // Not a struct, can't transform
    }
}

#[cfg(test)]
mod tests {

    use quote::ToTokens;

    use super::*;

    #[test]
    fn test_add_fiasko_error_derive_effect() {
        let mut input: DeriveInput = parse_quote! {
            struct TestError {
                message: String,
            }
        };
        crate::error_type_attr::add_fiasko_error_derive(&mut input);

        let expected: proc_macro2::TokenStream = parse_quote! {
            #[derive(ohno::Error)]
            struct TestError {
                message: String,
            }
        };

        assert_eq!(input.to_token_stream().to_string(), expected.to_string());
    }

    #[test]
    fn test_add_ohno_core_field_effect() {
        let mut input: DeriveInput = parse_quote! {
            struct TestError {
                message: String,
            }
        };

        crate::error_type_attr::add_ohno_core_field(&mut input);

        let expected: proc_macro2::TokenStream = parse_quote! {
            struct TestError {
                message: String,
                #[error]
                ohno_core: ohno::OhnoCore
            }
        };

        assert_eq!(input.to_token_stream().to_string(), expected.to_string());
    }

    #[test]
    fn test_add_ohno_core_field_with_enum() {
        // Test that the function doesn't crash when given an enum
        // This covers line 99 (the else branch for non-struct types)
        let mut input: DeriveInput = parse_quote! {
            enum TestError {
                Variant1,
                Variant2,
            }
        };

        let original = input.to_token_stream().to_string();
        crate::error_type_attr::add_ohno_core_field(&mut input);

        // The enum should remain unchanged since we can't transform it
        assert_eq!(input.to_token_stream().to_string(), original);
    }
}
