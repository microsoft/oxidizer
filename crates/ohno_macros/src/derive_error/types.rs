// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::collections::HashMap;

use quote::quote;

/// Backtrace capture policy for generated error types.
///
/// This enum controls how backtraces are captured when errors are created.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum BacktracePolicy {
    /// Automatically decide based on the RUST_BACKTRACE environment variable.
    #[default]
    Auto,
    /// Force backtrace capture even if the RUST_BACKTRACE environment variable is not set.
    Force,
    /// Never capture backtraces, regardless of the RUST_BACKTRACE environment variable.
    Disabled,
}

impl BacktracePolicy {
    /// Generate the token stream for creating an OhnoCore with this policy.
    pub fn to_builder_call(&self) -> proc_macro2::TokenStream {
        match self {
            Self::Auto => quote! { ohno::OhnoCore::default() },
            Self::Force => {
                quote! { ohno::OhnoCoreBuilder::new().backtrace_policy(ohno::BacktracePolicy::Forced).build() }
            }
            Self::Disabled => {
                quote! { ohno::OhnoCoreBuilder::new().backtrace_policy(ohno::BacktracePolicy::Disabled).build() }
            }
        }
    }

    /// Generate the token stream for creating an OhnoCore with this policy and an error.
    pub fn to_builder_call_with_error(&self) -> proc_macro2::TokenStream {
        match self {
            Self::Auto => quote! { ohno::OhnoCoreBuilder::new().error(error).build() },
            Self::Force => {
                quote! { ohno::OhnoCoreBuilder::new().backtrace_policy(ohno::BacktracePolicy::Forced).error(error).build() }
            }
            Self::Disabled => {
                quote! { ohno::OhnoCoreBuilder::new().backtrace_policy(ohno::BacktracePolicy::Disabled).error(error).build() }
            }
        }
    }
}

/// Represents different ways to access the error field in a struct
#[derive(Debug)]
pub enum ErrorFieldRef {
    /// Named field: `self.field_name`
    Named(syn::Ident),
    /// Tuple field: `self.0`, `self.1`, etc.
    Indexed(syn::Index),
}

impl ErrorFieldRef {
    /// Generate the token stream for accessing this field (e.g., `self.field_name` or `self.0`)
    pub fn to_field_access(&self) -> proc_macro2::TokenStream {
        match self {
            Self::Named(ident) => quote! { #ident },
            Self::Indexed(index) => quote! { #index },
        }
    }
}

impl std::fmt::Display for ErrorFieldRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Named(ident) => write!(f, "{ident}"),
            Self::Indexed(index) => write!(f, "{}", index.index),
        }
    }
}

/// Configuration for a single From implementation with optional field expressions
#[derive(Debug)]
pub struct FromConfig {
    /// The type to implement From for
    pub from_type: syn::Type,
    /// Custom field expressions: `field_name -> expression`
    pub field_expressions: HashMap<String, syn::Expr>,
}
