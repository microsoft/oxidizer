// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! This module contains helper functions for consuming and producing Rust syntax elements.

use proc_macro2::TokenStream;
use quote::quote;
use syn::spanned::Spanned;
use syn::{GenericArgument, PathArguments, Type};

/// Combines a token stream with a syn-originating contextual error message that contains
/// all the necessary metadata to emit rich errors (with red underlines and all that).
///
/// Also preserves the original token stream, merely appending the error instead of replacing.
#[must_use]
#[expect(
    clippy::needless_pass_by_value,
    reason = "Convention for syn-based code"
)]
pub fn token_stream_and_error(s: TokenStream, e: syn::Error) -> TokenStream {
    let error = e.to_compile_error();

    // We preserve both the original input and emit the compiler error message.
    // This ensures that we do not cause extra problems by removing the original input
    // from the code file (which would result in "trait not found" and similar errors).
    quote! {
        #s
        #error
    }
}

/// Return compiler error with a message at the specified span.
/// Use this macro to return a compiler error from a function that returns `TokenStream`.
/// If there is an input token stream that you want to return as is
/// (for example, when mutating existing code by attribute macro), add it as a first macro argument.
///
/// ## Examples
/// ```ignore
/// # use proc_macro2::TokenStream;
/// use syn::Stmt;
/// # use oxidizer_macros_impl::bail;
///
/// fn example() -> proc_macro2::TokenStream {
///     let span: Stmt = syn::parse_quote! { let x = 5; };
///     bail!(span, "This is a test error message.");
/// }
///
/// fn example2() -> proc_macro2::TokenStream {
///     let input_code = TokenStream::new();
///     let span: Stmt = syn::parse_quote! { let x = 5; };
///     bail!(input_code, span, "This is a test error message.");
///  }
///
/// # let result = example();
/// # let result = example2();
/// ```
///
macro_rules! bail {
    ($input:expr_2021, $span:expr_2021, $msg:expr_2021) => {{
        let error = ::syn::Error::new_spanned($span, $msg).to_compile_error();
        let input = $input;
        return ::quote::quote! {
            #input
            #error
        };
    }};
    ($span:expr_2021, $msg:expr_2021) => {{
        bail! { ::proc_macro2::TokenStream::new(), $span, $msg }
    }};
}
pub(crate) use bail;

/// Attempts to identify any compile-time error in the token stream. This is useful for unit
/// testing macros - if the macro is expected to produce a compile-time error, we can check
/// whether one exists.
///
/// We deliberately do not take an error message as input here. Testing for error messages is
/// fragile and creates maintenance headaches - be satisfied with OK/NOK testing and keep it simple.
#[cfg(test)]
#[must_use]
pub fn contains_compile_error(tokens: &TokenStream) -> bool {
    // String-based implementation, so vulnerable to false positives in very unlikely cases.
    tokens.to_string().contains(":: core :: compile_error ! {")
}

pub fn extract_inner_generic_type(type_path: &Type) -> Result<&Type, syn::Error> {
    if let Type::Path(type_path) = type_path {
        let path = &type_path.path;

        // We expect to have either Option<T> or Vec<T> or equivalent.
        if path.segments.len() != 1 {
            return Err(syn::Error::new(
                path.span(),
                "expected a single segment in the type path - multi::level::type::names are not supported",
            ));
        }

        let first_segment = path
            .segments
            .first()
            .expect("we already verified there is at least one segment");

        // Retrieve the generic argument (T).
        if let PathArguments::AngleBracketed(params) = &first_segment.arguments {
            if params.args.len() != 1 {
                return Err(syn::Error::new(
                    params.span(),
                    "expected a single generic argument in the type path",
                ));
            }

            if let Some(GenericArgument::Type(ty)) = params.args.first() {
                return Ok(ty);
            }
        }
    }

    Err(syn::Error::new(
        type_path.span(),
        "expected a type path with a single generic argument like Option<T> or Vec<T>",
    ))
}

#[cfg(test)]
mod tests {
    #[cfg(not(miri))] // Miri is not compatible with FFI calls this needs to make.
    use insta::assert_snapshot;
    use proc_macro2::Span;

    use super::*;

    #[test]
    fn token_stream_and_error_outputs_both() {
        // This is a bit tricky because we do not know the specific form the compiler error
        // is going to be. However, we know it must contain our error message, so just check that.
        let canary = "nrtfynjcrtupyh6rhdoj85m7yoi";

        // We also need to ensure it contains this function (that it did not get overwritten).
        let s = quote! {
            fn gkf5dj8yhuldri58uygdkiluyot() {}
        };

        let e = syn::Error::new(proc_macro2::Span::call_site(), canary);

        let merged = token_stream_and_error(s, e);

        let merged_str = merged.to_string();
        assert!(merged_str.contains(canary));
        assert!(merged_str.contains("gkf5dj8yhuldri58uygdkiluyot"));
    }

    #[test]
    fn contains_compile_error_yes_raw() {
        let tokens = quote! {
            let foo = "Some random stuff may also be here";
            blah! { blah }
            ::core::compile_error! { "This is a test error message." };
            let bar = "More random stuff here"
        };

        assert!(contains_compile_error(&tokens));
    }

    #[test]
    fn contains_compile_error_yes_generated() {
        let tokens = quote! {
            let foo = "Some random stuff may also be here";
            blah! { blah }
            ::core::compile_error!("This is a test error message.");
            let bar = "More random stuff here"
        };

        let tokens = token_stream_and_error(tokens, syn::Error::new(Span::call_site(), "Testing"));

        assert!(contains_compile_error(&tokens));
    }

    #[test]
    fn contains_compile_error_no() {
        let tokens = quote! {
            let foo = "No compile error here!"
        };

        assert!(!contains_compile_error(&tokens));
    }

    #[test]
    fn extract_inner_generic_type_success() {
        let ty = syn::parse_quote! { Option<u32> };
        let inner = extract_inner_generic_type(&ty).unwrap();
        assert_eq!(quote! { u32 }.to_string(), quote! { #inner }.to_string());

        let ty = syn::parse_quote! { Vec<&str> };
        let inner = extract_inner_generic_type(&ty).unwrap();
        assert_eq!(quote! { &str }.to_string(), quote! { #inner }.to_string());
    }

    #[test]
    fn extract_inner_generic_type_fail_non_generic_owned() {
        let ty = syn::parse_quote! { String };
        _ = extract_inner_generic_type(&ty).unwrap_err();
    }

    #[test]
    fn extract_inner_generic_type_fail_ref() {
        let ty = syn::parse_quote! { &Option<usize> };
        _ = extract_inner_generic_type(&ty).unwrap_err();
    }

    #[test]
    fn extract_inner_generic_type_fail_lifetime() {
        let ty = syn::parse_quote! { Option<'a> };
        _ = extract_inner_generic_type(&ty).unwrap_err();
    }

    #[test]
    fn extract_inner_generic_type_fail_too_many_parts() {
        let ty = syn::parse_quote! { std::option::Option<usize> };
        _ = extract_inner_generic_type(&ty).unwrap_err();
    }

    #[test]
    fn extract_inner_generic_type_fail_multiple_generic_params() {
        let ty = syn::parse_quote! { Result<usize, u32> };
        _ = extract_inner_generic_type(&ty).unwrap_err();
    }

    #[cfg(not(miri))] // Miri is not compatible with insta, used by `bail!`.
    #[test]
    fn bail_snapshot_simple() {
        fn bail_simple() -> TokenStream {
            let span: syn::Stmt = syn::parse_quote! { let x = 5; };
            bail!(span, "This is a test error message.");
        }
        assert_snapshot!(bail_simple(), @r#":: core :: compile_error ! { "This is a test error message." }"#);
    }

    #[cfg(not(miri))] // Miri is not compatible with insta, used by `bail!`.
    #[test]
    fn bail_snapshot_input() {
        fn bail_with_input() -> TokenStream {
            let input_code = quote! { let y = 10; };
            let span: syn::Stmt = syn::parse_quote! { let x = 5; };
            bail!(input_code, span, "This is a test error message.");
        }
        assert_snapshot!(bail_with_input(), @r#"let y = 10 ; :: core :: compile_error ! { "This is a test error message." }"#);
    }
}