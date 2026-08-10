// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use proc_macro::TokenStream;
use quote::quote;
use syn::{Data, DataStruct, DeriveInput, Fields, parse_macro_input, parse_quote};

use crate::derive_error::is_generated_error_field;
use crate::utils::{GENERATED_ERROR_FIELD_MARKER, generate_unique_field_name};

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
/// It can also be applied to existing structs with fields:
/// ```ignore
/// /// My awesome error
/// #[ohno::error]
/// #[derive(Debug)]
/// #[from(std::io::Error(kind: ErrorKind::Io))]
/// pub struct Error {
///     pub(crate) kind: ErrorKind,
/// }
/// ```
#[cfg_attr(test, mutants::skip)] // procedural macro API cannot be used in tests directly
pub(crate) fn error(_args: TokenStream, input: TokenStream) -> TokenStream {
    let mut input = parse_macro_input!(input as DeriveInput);
    TokenStream::from(error_impl(&mut input))
}

fn error_impl(input: &mut DeriveInput) -> proc_macro2::TokenStream {
    // The shape is settled once, here: every rejection below describes a struct, so on anything
    // else the accurate complaint is that the attribute does not apply at all.
    let Data::Struct(data_struct) = &mut input.data else {
        return syn::Error::new_spanned(&input.ident, NOT_A_STRUCT).to_compile_error();
    };

    if let Err(err) = validate(data_struct, &input.attrs) {
        return err.to_compile_error();
    }

    add_ohno_core_field(data_struct);
    add_fiasko_error_derive(input);

    quote! { #input }
}

/// Check everything this attribute refuses about a struct, in one place
///
/// See `docs/error_error.md`.
fn validate(data_struct: &DataStruct, attrs: &[syn::Attribute]) -> syn::Result<()> {
    reject_marked_field(data_struct)?;
    reject_generated_marker(data_struct)?;
    reject_no_constructors(attrs)
}

const NOT_A_STRUCT: &str = "#[ohno::error] can only be applied to structs";

const ALREADY_MARKED: &str = "`#[ohno::error]` adds the OhnoCore field itself and generates the error representation from it, so no field may be marked with `#[error]`. Remove the marker to keep the field as data, or use `#[derive(ohno::Error)]` to place the core explicitly";
const RESERVED_MARKER: &str = "This doc comment is reserved for `#[ohno::error]`, which puts it on the OhnoCore field it adds. Remove it; if this is the field holding the OhnoCore, use `#[derive(ohno::Error)]` and mark it with `#[error]`";
const NO_CONSTRUCTORS: &str = "`#[no_constructors]` is not supported under `#[ohno::error]`. A constructor has to initialize the OhnoCore field, and the field inserted by `#[ohno::error]` has no stable name, so it must not be referred to in code. Use `#[derive(ohno::Error)]` and declare the OhnoCore field explicitly";

/// Reject a struct that opts out of the generated constructors
///
/// See `docs/error_error.md`.
fn reject_no_constructors(attrs: &[syn::Attribute]) -> syn::Result<()> {
    match attrs.iter().find(|attr| attr.path().is_ident("no_constructors")) {
        Some(attr) => Err(syn::Error::new_spanned(attr, NO_CONSTRUCTORS)),
        None => Ok(()),
    }
}

/// Reject a struct that marks a field with `#[error]`
///
/// See `docs/error_error.md`.
fn reject_marked_field(data_struct: &DataStruct) -> syn::Result<()> {
    for field in &data_struct.fields {
        if let Some(attr) = field.attrs.iter().find(|attr| attr.path().is_ident("error")) {
            return Err(syn::Error::new_spanned(attr, ALREADY_MARKED));
        }
    }

    Ok(())
}

/// Reject a struct that already carries the marker this attribute writes
///
/// See `docs/error_error.md`.
fn reject_generated_marker(data_struct: &DataStruct) -> syn::Result<()> {
    for field in &data_struct.fields {
        if is_generated_error_field(field) {
            return Err(syn::Error::new_spanned(field, RESERVED_MARKER));
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

fn add_ohno_core_field(data_struct: &mut DataStruct) {
    let marker = GENERATED_ERROR_FIELD_MARKER;
    match &mut data_struct.fields {
        Fields::Unit => {
            // Unit struct: convert to tuple struct with OhnoCore
            let field: syn::Field = parse_quote! {
                #[doc = #marker] ohno::OhnoCore
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
                #[doc = #marker] ohno::OhnoCore
            });
        }
        Fields::Named(fields) => {
            let names = fields
                .named
                .iter()
                .map(|f| f.ident.as_ref().expect("Fields::Named always has idents"))
                .collect::<Vec<_>>();
            let field_name = generate_unique_field_name(&names);
            fields.named.push(parse_quote! {
                #[doc = #marker]
                #field_name: ohno::OhnoCore
            });
        }
    }
}

#[cfg(test)]
mod tests {

    use quote::ToTokens;

    use super::*;

    /// Every rejection is asserted through `error_impl`, the one entry point that settles the
    /// shape, so no test has to take a `DataStruct` apart
    fn expand(input: DeriveInput) -> String {
        let mut input = input;
        crate::error_type_attr::error_impl(&mut input).to_string()
    }

    #[test]
    fn test_reject_marked_field() {
        // The attribute generates the error representation from the field it injects, so a marker
        // on another field asks for something it cannot honor
        for input in [
            parse_quote! { struct TestError { path: String, #[error] inner: ohno::OhnoCore } },
            parse_quote! { struct TestError(String, #[error] ohno::OhnoCore); },
            parse_quote! { struct TestError { path: String, #[error] other: String } },
        ] {
            let input: DeriveInput = input;
            let expansion = expand(input);
            assert!(expansion.contains(crate::error_type_attr::ALREADY_MARKED), "got: {expansion}");
        }

        // A declared core field is an ordinary field, since the injected one is the marked one. A
        // struct without fields has nothing to reject
        for input in [
            parse_quote! { struct TestError { path: String } },
            parse_quote! { struct TestError { path: String, inner: ohno::OhnoCore } },
            parse_quote! { struct TestError(String, OhnoCore); },
            parse_quote! { struct TestError; },
        ] {
            let input: DeriveInput = input;
            let expansion = expand(input);
            assert!(!expansion.contains("compile_error"), "got: {expansion}");
        }
    }

    #[test]
    fn test_reject_generated_marker() {
        // The attribute has not added its field yet, so this marker was written by hand. One would
        // take over the error representation; two would settle it by declaration order
        let marker = GENERATED_ERROR_FIELD_MARKER;
        for input in [
            parse_quote! { struct TestError { path: String, #[doc = #marker] mine: ohno::OhnoCore } },
            parse_quote! { struct TestError(String, #[doc = #marker] ohno::OhnoCore); },
            parse_quote! { struct TestError { #[doc = #marker] a: ohno::OhnoCore, #[doc = #marker] b: ohno::OhnoCore } },
            parse_quote! { struct TestError { path: String, #[doc = #marker] other: String } },
        ] {
            let input: DeriveInput = input;
            let expansion = expand(input);
            assert!(expansion.contains(crate::error_type_attr::RESERVED_MARKER), "got: {expansion}");
        }

        // An ordinary doc comment is not the marker, and a struct without fields has nothing to
        // reject
        for input in [
            parse_quote! { struct TestError { #[doc = " The path."] path: String } },
            parse_quote! { struct TestError { path: String, inner: ohno::OhnoCore } },
            parse_quote! { struct TestError; },
        ] {
            let input: DeriveInput = input;
            let expansion = expand(input);
            assert!(!expansion.contains("compile_error"), "got: {expansion}");
        }
    }

    #[test]
    fn test_reject_no_constructors() {
        // Opting out of the generated constructors means writing the struct literal by hand, which
        // needs the name of the field this attribute adds
        for input in [
            parse_quote! { #[no_constructors] struct TestError { path: String } },
            parse_quote! { #[no_constructors] struct TestError(String); },
            parse_quote! { #[derive(Clone)] #[no_constructors] struct TestError; },
        ] {
            let input: DeriveInput = input;
            let expansion = expand(input);
            assert!(expansion.contains(crate::error_type_attr::NO_CONSTRUCTORS), "got: {expansion}");
        }

        // The attribute is rejected wherever it is written relative to `#[ohno::error]`, since
        // both orderings leave it in the item's attributes
        let input: DeriveInput = parse_quote! { #[no_constructors] #[derive(Clone)] struct TestError { path: String } };
        let expansion = expand(input);
        assert!(expansion.contains(crate::error_type_attr::NO_CONSTRUCTORS), "got: {expansion}");

        // Every other attribute is left alone
        for input in [
            parse_quote! { struct TestError { path: String } },
            parse_quote! { #[no_debug] #[display("boom")] struct TestError { path: String } },
        ] {
            let input: DeriveInput = input;
            let expansion = expand(input);
            assert!(!expansion.contains("compile_error"), "got: {expansion}");
        }
    }

    #[test]
    fn test_the_shape_is_settled_before_the_attributes() {
        // An enum cannot carry the added field at all, so it is told that rather than being given
        // advice about constructors it could never follow
        for input in [
            parse_quote! { enum TestError { A } },
            parse_quote! { #[no_constructors] enum TestError { A } },
            parse_quote! { #[no_constructors] union TestError { a: u32 } },
        ] {
            let input: DeriveInput = input;
            let expansion = expand(input);
            assert!(expansion.contains(crate::error_type_attr::NOT_A_STRUCT), "got: {expansion}");
        }
    }

    #[test]
    fn test_declared_core_field_is_left_to_the_user() {
        // The injected field carries the marker, so it is the one the implementations are
        // generated from, and the declared field survives untouched
        let mut input: DeriveInput = parse_quote! {
            struct TestError { path: String, inner: ohno::OhnoCore }
        };

        let expansion = crate::error_type_attr::error_impl(&mut input).to_string();

        let expected: proc_macro2::TokenStream = parse_quote! {
            #[derive(ohno::Error)]
            struct TestError {
                path: String,
                inner: ohno::OhnoCore,
                #[doc = " ohno::generated-core@7f3d9c2a"]
                ohno_core: ohno::OhnoCore
            }
        };

        assert_eq!(expansion, expected.to_string());
    }

    #[test]
    fn test_error_impl_reports_an_already_marked_field() {
        let mut input: DeriveInput = parse_quote! {
            struct TestError { #[error] inner: ohno::OhnoCore }
        };

        let expansion = crate::error_type_attr::error_impl(&mut input).to_string();
        assert!(expansion.contains("compile_error"), "expansion should be a compile error");
        assert!(expansion.contains("no field may be marked"), "got: {expansion}");
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

        let expansion = crate::error_type_attr::error_impl(&mut input).to_string();

        let expected: proc_macro2::TokenStream = parse_quote! {
            #[derive(ohno::Error)]
            struct TestError {
                message: String,
                #[doc = " ohno::generated-core@7f3d9c2a"]
                ohno_core: ohno::OhnoCore
            }
        };

        assert_eq!(expansion, expected.to_string());
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
