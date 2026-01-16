// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.


use quote::quote;

use super::*;
use crate::utils::assert_formatted_snapshot;

#[test]
fn preserves_pub_visibility() {
    let args = proc_macro2::TokenStream::new();
    let input: syn::ItemFn = syn::parse_quote! {
        pub fn test_function() -> Result<(), OhnoErrorType> {
            Ok(())
        }
    };

    let result = impl_enrich_err_attribute(args, input).unwrap();
    assert_formatted_snapshot!(result);
}

#[test]
fn preserves_pub_crate_visibility() {
    let args = proc_macro2::TokenStream::new();
    let input: syn::ItemFn = syn::parse_quote! {
        pub(crate) fn test_function() -> Result<(), OhnoErrorType> {
            Ok(())
        }
    };

    let result = impl_enrich_err_attribute(args, input).unwrap();
    assert_formatted_snapshot!(result);
}

#[test]
fn preserves_pub_super_visibility() {
    let args = proc_macro2::TokenStream::new();
    let input: syn::ItemFn = syn::parse_quote! {
        pub(super) fn test_function() -> Result<(), OhnoErrorType> {
            Ok(())
        }
    };

    let result = impl_enrich_err_attribute(args, input).unwrap();
    assert_formatted_snapshot!(result);
}

#[test]
fn preserves_const_modifier() {
    let args = proc_macro2::TokenStream::new();
    let input: syn::ItemFn = syn::parse_quote! {
        const fn test_function() -> Result<(), OhnoErrorType> {
            Ok(())
        }
    };

    let result = impl_enrich_err_attribute(args, input).unwrap();
    assert_formatted_snapshot!(result);
}

#[test]
fn preserves_unsafe_modifier() {
    let args = proc_macro2::TokenStream::new();
    let input: syn::ItemFn = syn::parse_quote! {
        unsafe fn test_function() -> Result<(), OhnoErrorType> {
            Ok(())
        }
    };

    let result = impl_enrich_err_attribute(args, input).unwrap();
    assert_formatted_snapshot!(result);
}

#[test]
fn preserves_generic_type_parameter() {
    let args = proc_macro2::TokenStream::new();
    let input: syn::ItemFn = syn::parse_quote! {
        fn test_function<T>(value: T) -> Result<(), OhnoErrorType> {
            Ok(())
        }
    };

    let result = impl_enrich_err_attribute(args, input).unwrap();
    assert_formatted_snapshot!(result);
}

#[test]
fn preserves_generic_with_trait_bound() {
    let args = proc_macro2::TokenStream::new();
    let input: syn::ItemFn = syn::parse_quote! {
        fn test_function<T: Display>(value: T) -> Result<(), OhnoErrorType> {
            Ok(())
        }
    };

    let result = impl_enrich_err_attribute(args, input).unwrap();
    assert_formatted_snapshot!(result);
}

#[test]
fn preserves_multiple_generics_with_bounds() {
    let args = proc_macro2::TokenStream::new();
    let input: syn::ItemFn = syn::parse_quote! {
        fn test_function<T: Display, U: Clone>(value: T, other: U) -> Result<(), OhnoErrorType> {
            Ok(())
        }
    };

    let result = impl_enrich_err_attribute(args, input).unwrap();
    assert_formatted_snapshot!(result);
}

#[test]
fn preserves_where_clause() {
    let args = proc_macro2::TokenStream::new();
    let input: syn::ItemFn = syn::parse_quote! {
        fn test_function<T>(value: T) -> Result<(), OhnoErrorType>
        where
            T: Display + Clone,
        {
            Ok(())
        }
    };

    let result = impl_enrich_err_attribute(args, input).unwrap();
    assert_formatted_snapshot!(result);
}

#[test]
fn preserves_complex_where_clause() {
    let args = proc_macro2::TokenStream::new();
    let input: syn::ItemFn = syn::parse_quote! {
        fn test_function<T, U>(value: T, other: U) -> Result<(), OhnoErrorType>
        where
            T: Display + Clone + Send,
            U: Into<String> + 'static,
        {
            Ok(())
        }
    };

    let result = impl_enrich_err_attribute(args, input).unwrap();
    assert_formatted_snapshot!(result);
}

#[test]
fn preserves_lifetime_parameters() {
    let args = proc_macro2::TokenStream::new();
    let input: syn::ItemFn = syn::parse_quote! {
        fn test_function<'a>(value: &'a str) -> Result<(), OhnoErrorType> {
            Ok(())
        }
    };

    let result = impl_enrich_err_attribute(args, input).unwrap();
    assert_formatted_snapshot!(result);
}

#[test]
fn preserves_multiple_lifetime_parameters() {
    let args = proc_macro2::TokenStream::new();
    let input: syn::ItemFn = syn::parse_quote! {
        fn test_function<'a, 'b>(first: &'a str, second: &'b str) -> Result<(), OhnoErrorType> {
            Ok(())
        }
    };

    let result = impl_enrich_err_attribute(args, input).unwrap();
    assert_formatted_snapshot!(result);
}

#[test]
fn preserves_lifetimes_and_generics_combined() {
    let args = proc_macro2::TokenStream::new();
    let input: syn::ItemFn = syn::parse_quote! {
        fn test_function<'a, T: Display>(value: &'a T) -> Result<(), OhnoErrorType> {
            Ok(())
        }
    };

    let result = impl_enrich_err_attribute(args, input).unwrap();
    assert_formatted_snapshot!(result);
}

#[test]
fn preserves_extern_abi() {
    let args = proc_macro2::TokenStream::new();
    let input: syn::ItemFn = syn::parse_quote! {
        extern "C" fn test_function() -> Result<(), OhnoErrorType> {
            Ok(())
        }
    };

    let result = impl_enrich_err_attribute(args, input).unwrap();
    assert_formatted_snapshot!(result);
}

#[test]
fn preserves_attributes() {
    let args = proc_macro2::TokenStream::new();
    let input: syn::ItemFn = syn::parse_quote! {
        #[inline]
        #[allow(clippy::unused)]
        fn test_function() -> Result<(), OhnoErrorType> {
            Ok(())
        }
    };

    let result = impl_enrich_err_attribute(args, input).unwrap();
    assert_formatted_snapshot!(result);
}

#[test]
fn preserves_doc_comments() {
    let args = proc_macro2::TokenStream::new();
    let input: syn::ItemFn = syn::parse_quote! {
        /// This is a test function.
        /// It does some work.
        fn test_function() -> Result<(), OhnoErrorType> {
            Ok(())
        }
    };

    let result = impl_enrich_err_attribute(args, input).unwrap();
    assert_formatted_snapshot!(result);
}

#[test]
fn preserves_impl_trait_params() {
    let args = proc_macro2::TokenStream::new();
    let input: syn::ItemFn = syn::parse_quote! {
        fn test_function(value: impl Display) -> Result<(), OhnoErrorType> {
            Ok(())
        }
    };

    let result = impl_enrich_err_attribute(args, input).unwrap();
    assert_formatted_snapshot!(result);
}

#[test]
fn preserves_impl_trait_with_bounds() {
    let args = proc_macro2::TokenStream::new();
    let input: syn::ItemFn = syn::parse_quote! {
        fn test_function(value: impl Display + Clone + Send) -> Result<(), OhnoErrorType> {
            Ok(())
        }
    };

    let result = impl_enrich_err_attribute(args, input).unwrap();
    assert_formatted_snapshot!(result);
}

#[test]
fn preserves_dyn_trait_params() {
    let args = proc_macro2::TokenStream::new();
    let input: syn::ItemFn = syn::parse_quote! {
        fn test_function(value: &dyn Display) -> Result<(), OhnoErrorType> {
            Ok(())
        }
    };

    let result = impl_enrich_err_attribute(args, input).unwrap();
    assert_formatted_snapshot!(result);
}

#[test]
fn preserves_all_features_combined() {
    let args: proc_macro2::TokenStream = quote! { "operation failed: {}", operation_name };
    let input: syn::ItemFn = syn::parse_quote! {
        /// Performs a complex operation.
        ///
        /// # Errors
        ///
        /// Returns an error if the operation fails.
        #[inline]
        #[allow(clippy::unused)]
        pub(crate) async unsafe fn complex_operation<'a, 'b, T, U>(
            &'a mut self,
            operation_name: &'b str,
            value: T,
            handler: impl Display + Send,
            callback: &dyn Fn() -> U,
        ) -> Result<(), OhnoErrorType>
        where
            T: Display + Clone + Send + 'static,
            U: Into<String> + Send,
        {
            let result = self.process(value).await?;
            callback();
            Ok(())
        }
    };

    let result = impl_enrich_err_attribute(args, input).unwrap();
    assert_formatted_snapshot!(result);
}

