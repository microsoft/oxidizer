// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![expect(missing_docs, reason = "Test code")]

use ohno_macros_impl::{derive_error, enrich_err, error};
use proc_macro2::TokenStream;
use quote::quote;

#[cfg(test)]
mod tests {
    use super::*;

    /// Whether an expansion is a diagnostic rather than generated code.
    ///
    /// The entry points only choose between expanding and reporting, so the two outcomes are all a
    /// test has to tell apart. The shape of what they expand to is covered by the per-module
    /// snapshot tests, which is why nothing here asserts on it.
    fn is_diagnostic(expanded: &TokenStream) -> bool {
        expanded.to_string().contains("compile_error")
    }

    #[test]
    fn derive_error_expands_a_parsable_input() {
        let expanded = derive_error(quote! {
            #[display("failed for {path}")]
            struct T { path: String, inner: ohno::OhnoCore }
        });
        assert!(!is_diagnostic(&expanded), "{expanded}");
    }

    #[test]
    fn derive_error_reports_an_unparsable_input() {
        let expanded = derive_error(quote!(
            fn not_a_type() {}
        ));
        assert!(is_diagnostic(&expanded), "{expanded}");
    }

    #[test]
    fn enrich_err_expands_a_parsable_item() {
        let expanded = enrich_err(
            TokenStream::new(),
            quote!(
                fn load() -> Result<(), MyError> {
                    Err(MyError::new())
                }
            ),
        );
        assert!(!is_diagnostic(&expanded), "{expanded}");
    }

    #[test]
    fn enrich_err_reports_an_unparsable_item() {
        let expanded = enrich_err(TokenStream::new(), quote!(1 + 1));
        assert!(is_diagnostic(&expanded), "{expanded}");
    }

    #[test]
    fn error_expands_a_parsable_item() {
        let expanded = error(
            TokenStream::new(),
            quote!(
                struct T {
                    path: String,
                }
            ),
        );
        assert!(!is_diagnostic(&expanded), "{expanded}");
    }

    #[test]
    fn error_reports_an_unparsable_item() {
        let expanded = error(TokenStream::new(), quote!(1 + 1));
        assert!(is_diagnostic(&expanded), "{expanded}");
    }

    #[test]
    fn error_rejects_any_argument() {
        let expanded = error(
            quote!(anything),
            quote!(
                struct T {
                    path: String,
                }
            ),
        );
        assert!(is_diagnostic(&expanded), "{expanded}");
        assert!(expanded.to_string().contains("takes no arguments"), "{expanded}");
    }
}
