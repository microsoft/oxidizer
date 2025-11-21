// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#[cfg(test)]
mod tests;

#[cfg(test)]
mod test_attrs;

use proc_macro::TokenStream;
use quote::quote;
use syn::{ItemFn, Result, parse_macro_input};

use crate::utils::bail;

/// Attribute macro for adding detailed error trace (with file and line info) to function errors.
///
/// Now supports complex format expressions like:
/// - `#[error_span("failed to read file: {}", path.display())]`
/// - `#[error_span("error in {}: {}", name, value.len())]`
/// - `#[error_span("simple message")]`
/// - `#[error_span("param interpolation: {param}")]`
///
/// See the main `ohno` crate documentation for detailed usage examples.
#[cfg_attr(test, mutants::skip)] // procedural macro API cannot be used in tests directly
pub fn error_span(args: TokenStream, input: TokenStream) -> TokenStream {
    let args = proc_macro2::TokenStream::from(args);
    let input = parse_macro_input!(input as ItemFn);

    impl_error_span_attribute(args, input)
        .unwrap_or_else(|err| err.to_compile_error())
        .into()
}

fn impl_error_span_attribute(trace_args: proc_macro2::TokenStream, mut fn_definition: ItemFn) -> Result<proc_macro2::TokenStream> {
    // Parse the arguments as either:
    // 1. A simple string literal: "message"
    // 2. A format string with args: "format {}", expr
    let trace_expr = if trace_args.is_empty() {
        // No arguments provided, use function name as default message
        let fn_name = &fn_definition.sig.ident;
        quote! { format!("error in function {}", stringify!(#fn_name)) }
    } else {
        generate_complex_context_expr(trace_args)?
    };

    check_return_type(&fn_definition.sig.output)?;
    let asyncness = &fn_definition.sig.asyncness;
    let await_suffix = asyncness.is_some().then(|| quote! { .await });
    let body = &fn_definition.block;

    let block = quote! {
        {
            (#asyncness || #body)() #await_suffix .map_err(|mut e| {
                let trace_msg = #trace_expr;
                ohno::ErrorSpan::add_error_span(&mut e, ohno::TraceInfo::detailed(trace_msg, file!(), line!()));
                e
            })
        }
    };

    fn_definition.block = syn::parse2(block)?;

    Ok(quote! { #fn_definition })
}

/// Generate error trace expression for complex format expressions.
/// Supports both simple string literals and format strings with complex expressions.
/// Also supports legacy-style parameter interpolation like "{param}".
pub fn generate_complex_context_expr(args_stream: proc_macro2::TokenStream) -> Result<proc_macro2::TokenStream> {
    // Parse the token stream - it could be:
    // 1. A single string literal: "message"
    // 2. A single string literal with parameter interpolation: "failed to read {path}"
    // 3. A format expression: "format {}", expr, expr2, ...

    let tokens: Vec<_> = args_stream.into_iter().collect();
    if tokens.is_empty() {
        bail!("error_span requires a message or format string");
    }

    // Check if it starts with a string literal
    let Some(proc_macro2::TokenTree::Literal(lit)) = tokens.first() else {
        bail!("cannot parse error_span arguments as a string literal or format expression");
    };
    let lit_str = lit.to_string();
    if !is_quoted_string(&lit_str) {
        bail!("error_span requires a string literal or format expression");
    }

    // Check if we have multiple tokens (format string with arguments) or interpolation
    if tokens.len() > 1 || (lit_str.contains('{') && lit_str.contains('}')) {
        let format_tokens = proc_macro2::TokenStream::from_iter(tokens);
        Ok(quote! { format!(#format_tokens) })
    } else {
        // Simple string literal - use it directly
        Ok(quote! { #lit })
    }
}

fn is_quoted_string(lit: &str) -> bool {
    lit.starts_with('"') && lit.ends_with('"')
}

/// Check that function returns a type
fn check_return_type(output: &syn::ReturnType) -> Result<()> {
    match output {
        syn::ReturnType::Type(_, _) => {
            // We don't really have a way to check if return type has `map_err` method here
            Ok(())
        }
        syn::ReturnType::Default => {
            bail!("context attribute can only be applied to functions returning Result")
        }
    }
}

#[cfg(test)]
mod inline_tests {
    use super::*;

    #[test]
    fn generate_complex_context_expr_simple() {
        let expr = generate_complex_context_expr(quote! { "simple message" }).unwrap();
        let expected = quote! { "simple message" };
        assert_eq!(expr.to_string(), expected.to_string());
    }

    #[test]
    fn generate_complex_context_expr_empty_args_stream() {
        let err = generate_complex_context_expr(proc_macro2::TokenStream::new()).unwrap_err();
        assert_eq!(err.to_string(), "error_span requires a message or format string");
    }

    #[test]
    fn generate_complex_context_expr_invalid_format() {
        // let format macro check for invalid format strings
        let expr = generate_complex_context_expr(quote! { "simple message", 123, 345 }).unwrap();
        let expected = quote! { format!("simple message", 123, 345) };
        assert_eq!(expr.to_string(), expected.to_string());
    }

    #[test]
    fn generate_complex_context_expr_with_one_brace() {
        let expr = generate_complex_context_expr(quote! { "simple {message" }).unwrap();
        let expected = quote! { "simple {message" };
        assert_eq!(expr.to_string(), expected.to_string());

        let expr = generate_complex_context_expr(quote! { "simple }message" }).unwrap();
        let expected = quote! { "simple }message" };
        assert_eq!(expr.to_string(), expected.to_string());
    }

    #[test]
    fn generate_complex_context_expr_format() {
        let err = generate_complex_context_expr(quote! { format!("error in {}", name) }).unwrap_err();
        let expected_err = "cannot parse error_span arguments as a string literal or format expression";
        assert_eq!(err.to_string(), expected_err);
    }

    #[test]
    fn generate_complex_context_expr_interpolation() {
        let expr = generate_complex_context_expr(quote! { "failed to read {path}" }).unwrap();
        let expected = quote! { format!("failed to read {path}") };
        assert_eq!(expr.to_string(), expected.to_string());
    }

    #[test]
    fn test_generate_complex_context_expr() {
        let expr = generate_complex_context_expr(quote! { "error in {}: {}", module, error_code }).unwrap();
        let expected = quote! { format!("error in {}: {}", module, error_code) };
        assert_eq!(expr.to_string(), expected.to_string());
    }

    #[test]
    fn generate_complex_context_expr_multiple_tokens_no_braces() {
        // This test ensures that format! is used when tokens.len() > 1, even without braces.
        // If the condition is changed from `> 1` to `== 1`, this test will fail because
        // the string "error occurred" has no braces, so it would be returned as a simple literal
        // instead of being wrapped in format!().
        let expr = generate_complex_context_expr(quote! { "error occurred", extra_arg }).unwrap();
        let expected = quote! { format!("error occurred", extra_arg) };
        assert_eq!(expr.to_string(), expected.to_string());
    }

    #[test]
    fn generate_complex_context_expr_invalid_literal() {
        // Test with a number literal instead of string literal
        let err = generate_complex_context_expr(quote! { 42 }).unwrap_err();
        let expected_err = "error_span requires a string literal or format expression";
        assert_eq!(err.to_string(), expected_err);
    }

    #[test]
    fn generate_complex_context_expr_boolean_literal() {
        // Test with a boolean literal instead of string literal
        // Boolean literals are parsed as identifiers, not literals, so they trigger the earlier error
        let err = generate_complex_context_expr(quote! { true }).unwrap_err();
        let expected_err = "cannot parse error_span arguments as a string literal or format expression";
        assert_eq!(err.to_string(), expected_err);
    }

    #[test]
    fn generate_complex_context_expr_char_literal() {
        // Test with a char literal instead of string literal
        let err = generate_complex_context_expr(quote! { 'c' }).unwrap_err();
        let expected_err = "error_span requires a string literal or format expression";
        assert_eq!(err.to_string(), expected_err);
    }

    #[test]
    fn test_is_quoted_string() {
        assert!(is_quoted_string("\"valid string\""));
        assert!(is_quoted_string("\"12345\""));
        assert!(is_quoted_string("\"true\""));
        assert!(!is_quoted_string("\"invalid string"));
        assert!(!is_quoted_string("invalid string\""));
        assert!(!is_quoted_string("12345"));
        assert!(!is_quoted_string("true"));
        assert!(!is_quoted_string("'single quoted'"));
    }

    #[test]
    fn check_return_type_with_result() {
        let return_type: syn::ReturnType = syn::parse_quote! { -> Result<(), String> };
        check_return_type(&return_type).unwrap();
    }

    #[test]
    fn check_return_type_without_result() {
        let return_type = syn::ReturnType::Default;
        let err = check_return_type(&return_type).unwrap_err();
        assert_eq!(
            err.to_string(),
            "context attribute can only be applied to functions returning Result"
        );
    }
}
