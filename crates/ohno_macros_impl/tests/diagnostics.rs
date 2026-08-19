// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![expect(missing_docs, reason = "Test code")]

use ohno_macros_impl::diagnostics::*;

#[cfg(test)]
mod tests {
    use quote::quote;

    use super::*;

    #[test]
    fn empty_accumulator_renders_nothing() {
        let errors = Errors::default();
        assert!(errors.is_empty());
        assert!(errors.into_compile_error().is_empty());
    }

    #[test]
    fn faults_accumulate_rather_than_replace() {
        let mut errors = Errors::default();
        errors.add(quote!(first), "first fault");
        assert!(!errors.is_empty());
        errors.add(quote!(second), "second fault");

        let rendered = errors.into_compile_error().to_string();
        assert!(rendered.contains("first fault"), "{rendered}");
        assert!(rendered.contains("second fault"), "{rendered}");
    }

    #[test]
    fn combine_accepts_a_prebuilt_error() {
        let mut errors = Errors::default();
        errors.combine(syn::Error::new_spanned(quote!(x), "parser fault"));
        assert!(errors.into_compile_error().to_string().contains("parser fault"));
    }
}
