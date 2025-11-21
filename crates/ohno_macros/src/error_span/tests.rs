// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use super::*;
use crate::utils::assert_token_streams_equal;

#[test]
fn empty_error_span() {
    let args = proc_macro2::TokenStream::new();
    let input: syn::ItemFn = syn::parse_quote! {
        fn test_function() -> Result<(), OhnoErrorType> {
            let x = 42;
            let y = x * 2;
            println!("Result: {}", y);
            Ok(())
        }
    };

    let result = impl_error_span_attribute(args, input).unwrap();

    let expected: proc_macro2::TokenStream = syn::parse_quote! {
        fn test_function() -> Result<(), OhnoErrorType> {
            (|| {
                let x = 42;
                let y = x * 2;
                println!("Result: {}", y);
                Ok(())
            })().map_err(|mut e| {
                let trace_msg = format!("error in function {}", stringify!(test_function));
                ohno::ErrorSpan::add_error_span(
                    &mut e,
                    ohno::SpanInfo::detailed(trace_msg, file!(), line!())
                );
                e
            })
        }
    };

    assert_token_streams_equal!(result, expected);
}

#[test]
fn empty_error_span_async() {
    let args = proc_macro2::TokenStream::new();
    let input: syn::ItemFn = syn::parse_quote! {
        async fn test_function() -> Result<(), OhnoErrorType> {
            let data = fetch_data().await?;
            process_data(&data);
            Ok(())
        }
    };

    let result = impl_error_span_attribute(args, input).unwrap();

    let expected: proc_macro2::TokenStream = syn::parse_quote! {
        async fn test_function() -> Result<(), OhnoErrorType> {
            (async || {
                let data = fetch_data().await?;
                process_data(&data);
                Ok(())
            })().await.map_err(|mut e| {
                let trace_msg = format!("error in function {}", stringify!(test_function));
                ohno::ErrorSpan::add_error_span(
                    &mut e,
                    ohno::SpanInfo::detailed(trace_msg, file!(), line!())
                );
                e
            })
        }
    };

    assert_token_streams_equal!(result, expected);
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

    let result = impl_error_span_attribute(args, input).unwrap();

    let expected: proc_macro2::TokenStream = syn::parse_quote! {
        fn test_function() -> Result<(), OhnoErrorType> {
            (|| {
                let value = compute_value()?;
                validate(value)?;
                Ok(())
            })().map_err(|mut e| {
                let trace_msg = "custom error message";
                ohno::ErrorSpan::add_error_span(
                    &mut e,
                    ohno::SpanInfo::detailed(trace_msg, file!(), line!())
                );
                e
            })
        }
    };

    assert_token_streams_equal!(result, expected);
}

#[test]
fn function_with_regular_string_async() {
    let args: proc_macro2::TokenStream = quote! { "custom error message" };
    let input: syn::ItemFn = syn::parse_quote! {
        async fn test_function() -> Result<(), OhnoErrorType> {
            Ok(())
        }
    };

    let result = impl_error_span_attribute(args, input).unwrap();

    let expected: proc_macro2::TokenStream = syn::parse_quote! {
        async fn test_function() -> Result<(), OhnoErrorType> {
            (async || { Ok(()) })().await.map_err(|mut e| {
                let trace_msg = "custom error message";
                ohno::ErrorSpan::add_error_span(
                    &mut e,
                    ohno::SpanInfo::detailed(trace_msg, file!(), line!())
                );
                e
            })
        }
    };

    assert_token_streams_equal!(result, expected);
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

    let result = impl_error_span_attribute(args, input).unwrap();

    let expected: proc_macro2::TokenStream = syn::parse_quote! {
        fn test_function(code: i32) -> Result<(), OhnoErrorType> {
            (|| {
                if code < 0 {
                    return Err(invalid_code_error());
                }
                process_code(code)?;
                Ok(())
            })().map_err(|mut e| {
                let trace_msg = format!("error code: {code}");
                ohno::ErrorSpan::add_error_span(
                    &mut e,
                    ohno::SpanInfo::detailed(trace_msg, file!(), line!())
                );
                e
            })
        }
    };

    assert_token_streams_equal!(result, expected);
}

#[test]
fn function_with_inline_format_async() {
    let args: proc_macro2::TokenStream = quote! { "error code: {code}" };
    let input: syn::ItemFn = syn::parse_quote! {
        async fn test_function(code: i32) -> Result<(), OhnoErrorType> {
            Ok(())
        }
    };

    let result = impl_error_span_attribute(args, input).unwrap();

    let expected: proc_macro2::TokenStream = syn::parse_quote! {
        async fn test_function(code: i32) -> Result<(), OhnoErrorType> {
            (async || { Ok(()) })().await.map_err(|mut e| {
                let trace_msg = format!("error code: {code}");
                ohno::ErrorSpan::add_error_span(
                    &mut e,
                    ohno::SpanInfo::detailed(trace_msg, file!(), line!())
                );
                e
            })
        }
    };

    assert_token_streams_equal!(result, expected);
}

#[test]
fn function_with_complex_format_expression() {
    let args: proc_macro2::TokenStream = quote! { "failed to read a file {}", path.as_ref().display() };
    let input: syn::ItemFn = syn::parse_quote! {
        fn test_function(path: &std::path::Path) -> Result<(), OhnoErrorType> {
            Ok(())
        }
    };

    let result = impl_error_span_attribute(args, input).unwrap();

    let expected: proc_macro2::TokenStream = syn::parse_quote! {
        fn test_function(path: &std::path::Path) -> Result<(), OhnoErrorType> {
            (|| { Ok(()) })().map_err(|mut e| {
                let trace_msg = format!("failed to read a file {}", path.as_ref().display());
                ohno::ErrorSpan::add_error_span(
                    &mut e,
                    ohno::SpanInfo::detailed(trace_msg, file!(), line!())
                );
                e
            })
        }
    };

    assert_token_streams_equal!(result, expected);
}

#[test]
fn function_with_complex_format_expression_async() {
    let args: proc_macro2::TokenStream = quote! { "failed to read a file {}", path.as_ref().display() };
    let input: syn::ItemFn = syn::parse_quote! {
        async fn test_function(path: &std::path::Path) -> Result<(), OhnoErrorType> {
            Ok(())
        }
    };

    let result = impl_error_span_attribute(args, input).unwrap();

    let expected: proc_macro2::TokenStream = syn::parse_quote! {
        async fn test_function(path: &std::path::Path) -> Result<(), OhnoErrorType> {
            (async || { Ok(()) })().await.map_err(|mut e| {
                let trace_msg = format!("failed to read a file {}", path.as_ref().display());
                ohno::ErrorSpan::add_error_span(
                    &mut e,
                    ohno::SpanInfo::detailed(trace_msg, file!(), line!())
                );
                e
            })
        }
    };

    assert_token_streams_equal!(result, expected);
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

    let result = impl_error_span_attribute(args, input).unwrap();

    let expected: proc_macro2::TokenStream = syn::parse_quote! {
        fn test_method(&self) -> Result<(), OhnoErrorType> {
            (|| {
                let state = self.get_state()?;
                state.validate()?;
                Ok(())
            })().map_err(|mut e| {
                let trace_msg = format!("error in function {}", stringify!(test_method));
                ohno::ErrorSpan::add_error_span(
                    &mut e,
                    ohno::SpanInfo::detailed(trace_msg, file!(), line!())
                );
                e
            })
        }
    };

    assert_token_streams_equal!(result, expected);
}

#[test]
fn method_with_self_ref_async() {
    let args = proc_macro2::TokenStream::new();
    let input: syn::ItemFn = syn::parse_quote! {
        async fn test_method(&self) -> Result<(), OhnoErrorType> {
            Ok(())
        }
    };

    let result = impl_error_span_attribute(args, input).unwrap();

    let expected: proc_macro2::TokenStream = syn::parse_quote! {
        async fn test_method(&self) -> Result<(), OhnoErrorType> {
            (async || { Ok(()) })().await.map_err(|mut e| {
                let trace_msg = format!("error in function {}", stringify!(test_method));
                ohno::ErrorSpan::add_error_span(
                    &mut e,
                    ohno::SpanInfo::detailed(trace_msg, file!(), line!())
                );
                e
            })
        }
    };

    assert_token_streams_equal!(result, expected);
}

#[test]
fn method_with_mut_self_ref() {
    let args = proc_macro2::TokenStream::new();
    let input: syn::ItemFn = syn::parse_quote! {
        fn test_method(&mut self) -> Result<(), OhnoErrorType> {
            Ok(())
        }
    };

    let result = impl_error_span_attribute(args, input).unwrap();

    let expected: proc_macro2::TokenStream = syn::parse_quote! {
        fn test_method(&mut self) -> Result<(), OhnoErrorType> {
            (|| { Ok(()) })().map_err(|mut e| {
                let trace_msg = format!("error in function {}", stringify!(test_method));
                ohno::ErrorSpan::add_error_span(
                    &mut e,
                    ohno::SpanInfo::detailed(trace_msg, file!(), line!())
                );
                e
            })
        }
    };

    assert_token_streams_equal!(result, expected);
}

#[test]
fn method_with_mut_self_ref_async() {
    let args = proc_macro2::TokenStream::new();
    let input: syn::ItemFn = syn::parse_quote! {
        async fn test_method(&mut self) -> Result<(), OhnoErrorType> {
            Ok(())
        }
    };

    let result = impl_error_span_attribute(args, input).unwrap();

    let expected: proc_macro2::TokenStream = syn::parse_quote! {
        async fn test_method(&mut self) -> Result<(), OhnoErrorType> {
            (async || { Ok(()) })().await.map_err(|mut e| {
                let trace_msg = format!("error in function {}", stringify!(test_method));
                ohno::ErrorSpan::add_error_span(
                    &mut e,
                    ohno::SpanInfo::detailed(trace_msg, file!(), line!())
                );
                e
            })
        }
    };

    assert_token_streams_equal!(result, expected);
}

#[test]
fn method_with_self_ref_formatting_self() {
    let args: proc_macro2::TokenStream = quote! { "operation failed, id: {}", self.id };
    let input: syn::ItemFn = syn::parse_quote! {
        fn test_method(&self) -> Result<(), OhnoErrorType> {
            Ok(())
        }
    };

    let result = impl_error_span_attribute(args, input).unwrap();

    let expected: proc_macro2::TokenStream = syn::parse_quote! {
        fn test_method(&self) -> Result<(), OhnoErrorType> {
            (|| { Ok(()) })().map_err(|mut e| {
                let trace_msg = format!("operation failed, id: {}", self.id);
                ohno::ErrorSpan::add_error_span(
                    &mut e,
                    ohno::SpanInfo::detailed(trace_msg, file!(), line!())
                );
                e
            })
        }
    };

    assert_token_streams_equal!(result, expected);
}

#[test]
fn method_with_self_ref_formatting_self_async() {
    let args: proc_macro2::TokenStream = quote! { "operation failed, id: {}", self.id };
    let input: syn::ItemFn = syn::parse_quote! {
        async fn test_method(&self) -> Result<(), OhnoErrorType> {
            Ok(())
        }
    };

    let result = impl_error_span_attribute(args, input).unwrap();

    let expected: proc_macro2::TokenStream = syn::parse_quote! {
        async fn test_method(&self) -> Result<(), OhnoErrorType> {
            (async || { Ok(()) })().await.map_err(|mut e| {
                let trace_msg = format!("operation failed, id: {}", self.id);
                ohno::ErrorSpan::add_error_span(
                    &mut e,
                    ohno::SpanInfo::detailed(trace_msg, file!(), line!())
                );
                e
            })
        }
    };

    assert_token_streams_equal!(result, expected);
}

#[test]
fn method_with_mut_self_ref_formatting_self() {
    let args: proc_macro2::TokenStream = quote! { "operation failed, id: {}", self.id };
    let input: syn::ItemFn = syn::parse_quote! {
        fn test_method(&mut self) -> Result<(), OhnoErrorType> {
            Ok(())
        }
    };

    let result = impl_error_span_attribute(args, input).unwrap();

    let expected: proc_macro2::TokenStream = syn::parse_quote! {
        fn test_method(&mut self) -> Result<(), OhnoErrorType> {
            (|| { Ok(()) })().map_err(|mut e| {
                let trace_msg = format!("operation failed, id: {}", self.id);
                ohno::ErrorSpan::add_error_span(
                    &mut e,
                    ohno::SpanInfo::detailed(trace_msg, file!(), line!())
                );
                e
            })
        }
    };

    assert_token_streams_equal!(result, expected);
}

#[test]
fn method_with_mut_self_ref_formatting_self_async() {
    let args: proc_macro2::TokenStream = quote! { "operation failed, id: {}", self.id };
    let input: syn::ItemFn = syn::parse_quote! {
        async fn test_method(&mut self) -> Result<(), OhnoErrorType> {
            Ok(())
        }
    };

    let result = impl_error_span_attribute(args, input).unwrap();

    let expected: proc_macro2::TokenStream = syn::parse_quote! {
        async fn test_method(&mut self) -> Result<(), OhnoErrorType> {
            (async || { Ok(()) })().await.map_err(|mut e| {
                let trace_msg = format!("operation failed, id: {}", self.id);
                ohno::ErrorSpan::add_error_span(
                    &mut e,
                    ohno::SpanInfo::detailed(trace_msg, file!(), line!())
                );
                e
            })
        }
    };

    assert_token_streams_equal!(result, expected);
}
