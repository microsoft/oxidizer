// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Accumulated diagnostics.
//!
//! A macro reports every fault it can see rather than the first, so faults are collected here and
//! rendered together at the entry point.

use std::fmt::Display;

use proc_macro2::TokenStream;
use quote::ToTokens;

/// Zero or more faults, reported together.
///
/// [`Errors::add`] takes tokens rather than a `Span`, so every diagnostic is anchored with
/// [`syn::Error::new_spanned`]. A span taken from a node covers the whole node only where
/// `Span::join` is available and shrinks to the first token elsewhere, which would make the same
/// diagnostic underline different amounts of code on different toolchains.
#[derive(Default)]
pub(crate) struct Errors(Option<syn::Error>);

impl Errors {
    /// Records a fault anchored at `tokens`.
    pub(crate) fn add(&mut self, tokens: impl ToTokens, message: impl Display) {
        self.combine(syn::Error::new_spanned(tokens, message));
    }

    /// Records an already-built error, such as one returned by a `syn` parser.
    pub(crate) fn combine(&mut self, error: syn::Error) {
        match &mut self.0 {
            Some(existing) => existing.combine(error),
            None => self.0 = Some(error),
        }
    }

    /// Returns `true` when nothing has been recorded.
    pub(crate) fn is_empty(&self) -> bool {
        self.0.is_none()
    }

    /// Renders everything recorded as `compile_error!` invocations.
    ///
    /// Returns an empty stream when nothing was recorded.
    pub(crate) fn into_compile_error(self) -> TokenStream {
        self.0.map(|error| error.to_compile_error()).unwrap_or_default()
    }
}

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
