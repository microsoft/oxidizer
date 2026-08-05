// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use proc_macro::TokenStream;
use quote::quote;
use syn::{Data, DeriveInput, Fields, parse_macro_input, parse_quote};

use crate::utils::{generate_unique_field_name, generated_error_field_marker};

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
pub(crate) fn error(_args: TokenStream, input: TokenStream) -> TokenStream {
    let mut input = parse_macro_input!(input as DeriveInput);
    TokenStream::from(error_impl(&mut input))
}

fn error_impl(input: &mut DeriveInput) -> proc_macro2::TokenStream {
    if let Err(err) = reject_marked_field(input) {
        return err.to_compile_error();
    }
    if let Err(err) = add_ohno_core_field(input) {
        return err.to_compile_error();
    }
    add_fiasko_error_derive(input);

    quote! { #input }
}

const ALREADY_MARKED: &str = "`#[ohno::error]` adds the OhnoCore field itself and generates the error representation from it, so no other field can be marked with `#[error]`. Use `#[derive(ohno::Error)]` to place the field yourself";

/// Reject a struct that marks a field with `#[error]`
///
/// `#[ohno::error]` always adds the `OhnoCore` field and always generates the error representation
/// from it, so a marker on another field asks for something the attribute cannot honour. Placing
/// the field by hand is what `#[derive(ohno::Error)]` is for.
///
/// Declaring a field of type `OhnoCore` is not rejected: the injected field is marked, so it is the
/// one the implementations are generated from, and the declared field stays an ordinary field the
/// user is free to reference.
fn reject_marked_field(input: &DeriveInput) -> syn::Result<()> {
    let Data::Struct(data_struct) = &input.data else {
        return Ok(());
    };

    for field in &data_struct.fields {
        if let Some(attr) = field.attrs.iter().find(|attr| attr.path().is_ident("error")) {
            return Err(syn::Error::new_spanned(attr, ALREADY_MARKED));
        }
    }

    Ok(())
}

fn add_fiasko_error_derive(input: &mut DeriveInput) {
    input.attrs.insert(
        0,
        parse_quote! {
            #[derive(ohno::Error)]
        },
    );
}

fn add_ohno_core_field(input: &mut DeriveInput) -> syn::Result<()> {
    if let Data::Struct(data_struct) = &mut input.data {
        let marker = generated_error_field_marker();
        match &mut data_struct.fields {
            Fields::Unit => {
                // Unit struct: convert to tuple struct with OhnoCore
                let field: syn::Field = parse_quote! {
                    #[error(#marker)] ohno::OhnoCore
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
                    #[error(#marker)] ohno::OhnoCore
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
                    #[error(#marker)]
                    #field_name: ohno::OhnoCore
                });
            }
        }
        Ok(())
    } else {
        Err(syn::Error::new_spanned(
            &input.ident,
            "#[ohno::error] can only be applied to structs",
        ))
    }
}

#[cfg(test)]
mod tests {

    use quote::ToTokens;

    use super::*;

    #[test]
    fn test_reject_marked_field() {
        // The attribute generates the error representation from the field it injects, so a marker
        // on another field asks for something it cannot honour
        for input in [
            parse_quote! { struct TestError { path: String, #[error] inner: ohno::OhnoCore } },
            parse_quote! { struct TestError(String, #[error] ohno::OhnoCore); },
            parse_quote! { struct TestError { path: String, #[error] other: String } },
        ] {
            let input: DeriveInput = input;
            let err = crate::error_type_attr::reject_marked_field(&input).unwrap_err();
            assert_eq!(err.to_string(), crate::error_type_attr::ALREADY_MARKED);
        }

        // A declared core field is an ordinary field, since the injected one is the marked one. An
        // input that cannot carry fields has nothing to reject
        for input in [
            parse_quote! { struct TestError { path: String } },
            parse_quote! { struct TestError { path: String, inner: ohno::OhnoCore } },
            parse_quote! { struct TestError(String, OhnoCore); },
            parse_quote! { struct TestError; },
            parse_quote! { enum TestError { A } },
        ] {
            let input: DeriveInput = input;
            crate::error_type_attr::reject_marked_field(&input).unwrap();
        }
    }

    #[test]
    fn test_declared_core_field_is_left_to_the_user() {
        // The injected field carries the marker, so it is the one the implementations are
        // generated from, and the declared field survives untouched
        let mut input: DeriveInput = parse_quote! {
            struct TestError { path: String, inner: ohno::OhnoCore }
        };

        crate::error_type_attr::add_ohno_core_field(&mut input).unwrap();

        let expected: proc_macro2::TokenStream = parse_quote! {
            struct TestError {
                path: String,
                inner: ohno::OhnoCore,
                #[error(generated)]
                ohno_core: ohno::OhnoCore
            }
        };

        assert_eq!(input.to_token_stream().to_string(), expected.to_string());
    }

    #[test]
    fn test_error_impl_reports_an_already_marked_field() {
        let mut input: DeriveInput = parse_quote! {
            struct TestError { #[error] inner: ohno::OhnoCore }
        };

        let expansion = crate::error_type_attr::error_impl(&mut input).to_string();
        assert!(expansion.contains("compile_error"), "expansion should be a compile error");
        assert!(expansion.contains("no other field can be marked"), "got: {expansion}");
    }

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

        crate::error_type_attr::add_ohno_core_field(&mut input).unwrap();

        let expected: proc_macro2::TokenStream = parse_quote! {
            struct TestError {
                message: String,
                #[error(generated)]
                ohno_core: ohno::OhnoCore
            }
        };

        assert_eq!(input.to_token_stream().to_string(), expected.to_string());
    }

    #[test]
    fn test_add_ohno_core_field_with_enum() {
        // Test that the function returns an error when given an enum
        let mut input: DeriveInput = parse_quote! {
            enum TestError {
                Variant1,
                Variant2,
            }
        };

        let result = crate::error_type_attr::add_ohno_core_field(&mut input);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("#[ohno::error] can only be applied to structs"));
    }

    #[test]
    fn test_add_ohno_core_field_enum_produces_compile_error() {
        // Verifies the to_compile_error() path used by the error proc macro (line 50)
        let mut input: DeriveInput = parse_quote! {
            enum NotAStruct {
                A,
            }
        };

        let err = crate::error_type_attr::add_ohno_core_field(&mut input).unwrap_err();
        let compile_error = err.to_compile_error().to_string();

        assert!(compile_error.contains("compile_error"));
        assert!(
            compile_error.contains("#[ohno::error] can only be applied to structs"),
            "compile error should contain the expected message, got: {compile_error}"
        );
    }

    #[test]
    fn test_error_impl_returns_compile_error_for_enum() {
        let mut input: DeriveInput = parse_quote! {
            enum NotAStruct {
                A,
            }
        };

        let output = crate::error_type_attr::error_impl(&mut input).to_string();

        assert!(
            output.contains("compile_error"),
            "error_impl should return a compile_error token stream for enums, got: {output}"
        );
    }
}
