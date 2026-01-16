// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.


use quote::quote;

use super::*;
use crate::utils::assert_formatted_snapshot;

#[test]
fn empty_enrich_err() {
    let args = proc_macro2::TokenStream::new();
    let input: syn::ItemFn = syn::parse_quote! {
        fn test_function() -> Result<(), OhnoErrorType> {
            let x = 42;
            let y = x * 2;
            println!("Result: {}", y);
            Ok(())
        }
    };

    let result = impl_enrich_err_attribute(args, input).unwrap();
    assert_formatted_snapshot!(result);
}

#[test]
fn empty_enrich_err_async() {
    let args = proc_macro2::TokenStream::new();
    let input: syn::ItemFn = syn::parse_quote! {
        async fn test_function() -> Result<(), OhnoErrorType> {
            let data = fetch_data().await?;
            process_data(&data);
            Ok(())
        }
    };

    let result = impl_enrich_err_attribute(args, input).unwrap();
    assert_formatted_snapshot!(result);
}

#[test]
fn function_with_regular_string() {
    let args: proc_macro2::TokenStream = quote! { "custom error message" };
    let input: syn::ItemFn = syn::parse_quote! {
        fn test_function() -> Result<(), OhnoErrorType> {
            let value = compute_value()?;
            validate(value)?;
            Ok(())
        }
    };

    let result = impl_enrich_err_attribute(args, input).unwrap();
    assert_formatted_snapshot!(result);
}

#[test]
fn function_with_regular_string_async() {
    let args: proc_macro2::TokenStream = quote! { "custom error message" };
    let input: syn::ItemFn = syn::parse_quote! {
        async fn test_function() -> Result<(), OhnoErrorType> {
            Ok(())
        }
    };

    let result = impl_enrich_err_attribute(args, input).unwrap();
    assert_formatted_snapshot!(result);
}

#[test]
fn function_with_inline_format() {
    let args: proc_macro2::TokenStream = quote! { "error code: {code}" };
    let input: syn::ItemFn = syn::parse_quote! {
        fn test_function(code: i32) -> Result<(), OhnoErrorType> {
            if code < 0 {
                return Err(invalid_code_error());
            }
            process_code(code)?;
            Ok(())
        }
    };

    let result = impl_enrich_err_attribute(args, input).unwrap();
    assert_formatted_snapshot!(result);
}

#[test]
fn function_with_inline_format_async() {
    let args: proc_macro2::TokenStream = quote! { "error code: {code}" };
    let input: syn::ItemFn = syn::parse_quote! {
        async fn test_function(code: i32) -> Result<(), OhnoErrorType> {
            Ok(())
        }
    };

    let result = impl_enrich_err_attribute(args, input).unwrap();
    assert_formatted_snapshot!(result);
}

#[test]
fn function_with_complex_format_expression() {
    let args: proc_macro2::TokenStream = quote! { "failed to read a file {}", path.as_ref().display() };
    let input: syn::ItemFn = syn::parse_quote! {
        fn test_function(path: &std::path::Path) -> Result<(), OhnoErrorType> {
            Ok(())
        }
    };

    let result = impl_enrich_err_attribute(args, input).unwrap();
    assert_formatted_snapshot!(result);
}

#[test]
fn function_with_complex_format_expression_async() {
    let args: proc_macro2::TokenStream = quote! { "failed to read a file {}", path.as_ref().display() };
    let input: syn::ItemFn = syn::parse_quote! {
        async fn test_function(path: &std::path::Path) -> Result<(), OhnoErrorType> {
            Ok(())
        }
    };

    let result = impl_enrich_err_attribute(args, input).unwrap();
    assert_formatted_snapshot!(result);
}

#[test]
fn method_with_self_ref() {
    let args = proc_macro2::TokenStream::new();
    let input: syn::ItemFn = syn::parse_quote! {
        fn test_method(&self) -> Result<(), OhnoErrorType> {
            let state = self.get_state()?;
            state.validate()?;
            Ok(())
        }
    };

    let result = impl_enrich_err_attribute(args, input).unwrap();
    assert_formatted_snapshot!(result);
}

#[test]
fn method_with_self_ref_async() {
    let args = proc_macro2::TokenStream::new();
    let input: syn::ItemFn = syn::parse_quote! {
        async fn test_method(&self) -> Result<(), OhnoErrorType> {
            Ok(())
        }
    };

    let result = impl_enrich_err_attribute(args, input).unwrap();
    assert_formatted_snapshot!(result);
}

#[test]
fn method_with_mut_self_ref() {
    let args = proc_macro2::TokenStream::new();
    let input: syn::ItemFn = syn::parse_quote! {
        fn test_method(&mut self) -> Result<(), OhnoErrorType> {
            Ok(())
        }
    };

    let result = impl_enrich_err_attribute(args, input).unwrap();
    assert_formatted_snapshot!(result);
}

#[test]
fn method_with_mut_self_ref_async() {
    let args = proc_macro2::TokenStream::new();
    let input: syn::ItemFn = syn::parse_quote! {
        async fn test_method(&mut self) -> Result<(), OhnoErrorType> {
            Ok(())
        }
    };

    let result = impl_enrich_err_attribute(args, input).unwrap();
    assert_formatted_snapshot!(result);
}

#[test]
fn method_with_self_ref_formatting_self() {
    let args: proc_macro2::TokenStream = quote! { "operation failed, id: {}", self.id };
    let input: syn::ItemFn = syn::parse_quote! {
        fn test_method(&self) -> Result<(), OhnoErrorType> {
            Ok(())
        }
    };

    let result = impl_enrich_err_attribute(args, input).unwrap();
    assert_formatted_snapshot!(result);
}

#[test]
fn method_with_self_ref_formatting_self_async() {
    let args: proc_macro2::TokenStream = quote! { "operation failed, id: {}", self.id };
    let input: syn::ItemFn = syn::parse_quote! {
        async fn test_method(&self) -> Result<(), OhnoErrorType> {
            Ok(())
        }
    };

    let result = impl_enrich_err_attribute(args, input).unwrap();
    assert_formatted_snapshot!(result);
}

#[test]
fn method_with_mut_self_ref_formatting_self() {
    let args: proc_macro2::TokenStream = quote! { "operation failed, id: {}", self.id };
    let input: syn::ItemFn = syn::parse_quote! {
        fn test_method(&mut self) -> Result<(), OhnoErrorType> {
            Ok(())
        }
    };

    let result = impl_enrich_err_attribute(args, input).unwrap();
    assert_formatted_snapshot!(result);
}

#[test]
fn method_with_mut_self_ref_formatting_self_async() {
    let args: proc_macro2::TokenStream = quote! { "operation failed, id: {}", self.id };
    let input: syn::ItemFn = syn::parse_quote! {
        async fn test_method(&mut self) -> Result<(), OhnoErrorType> {
            Ok(())
        }
    };

    let result = impl_enrich_err_attribute(args, input).unwrap();
    assert_formatted_snapshot!(result);
}

