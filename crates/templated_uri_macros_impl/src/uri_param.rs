// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use proc_macro2::TokenStream;
use quote::quote;
use syn::{Data, DeriveInput, Fields};

use crate::bail;

/// Generates the `UriParam` trait implementation for a newtype struct.
#[cfg_attr(test, mutants::skip)] // not relevant for auto-generated proc macros
pub(crate) fn uri_param_impl(input: DeriveInput) -> TokenStream {
    let ident = &input.ident;

    if !input.generics.params.is_empty() {
        return syn::Error::new_spanned(&input.generics, "UriParam cannot be derived for generic types").to_compile_error();
    }

    // Only support tuple structs (newtype pattern)
    let fields = match input.data {
        Data::Struct(ref data) => match data.fields {
            Fields::Unnamed(ref fields) => fields,
            _ => {
                bail!(input, "UriParam can only be derived for tuple structs (newtype pattern)");
            }
        },
        Data::Enum(_) => {
            bail!(input, "UriParam cannot be derived for enums");
        }
        Data::Union(_) => {
            bail!(input, "UriParam cannot be derived for unions");
        }
    };

    // Ensure exactly one field
    let field_count = fields.unnamed.len();
    if field_count != 1 {
        bail!(fields, "UriParam requires exactly one field, found {}", field_count);
    }

    // Generate the implementation
    quote! {
        impl ::templated_uri::UriParam for #ident {
            fn as_uri_escaped(&self) -> ::templated_uri::UriEscaped<impl ::std::fmt::Display> {
                self.0.as_uri_escaped()
            }
        }
    }
}

/// Generates the `UriUnsafeParam` trait implementation for a newtype struct.
pub(crate) fn uri_unsafe_param_impl(input: DeriveInput) -> TokenStream {
    let ident = &input.ident;

    if !input.generics.params.is_empty() {
        return syn::Error::new_spanned(&input.generics, "UriUnsafeParam cannot be derived for generic types").to_compile_error();
    }

    // Only support tuple structs (newtype pattern)
    let fields = match input.data {
        Data::Struct(ref data) => match data.fields {
            Fields::Unnamed(ref fields) => fields,
            _ => {
                bail!(input, "UriUnsafeParam can only be derived for tuple structs (newtype pattern)");
            }
        },
        Data::Enum(_) => {
            bail!(input, "UriUnsafeParam cannot be derived for enums");
        }
        Data::Union(_) => {
            bail!(input, "UriUnsafeParam cannot be derived for unions");
        }
    };

    // Ensure exactly one field
    let field_count = fields.unnamed.len();
    if field_count != 1 {
        bail!(fields, "UriUnsafeParam requires exactly one field, found {}", field_count);
    }

    // Generate the implementation
    quote! {
        impl ::templated_uri::UriUnsafeParam for #ident {
            fn as_display(&self) -> impl ::std::fmt::Display {
                &self.0
            }
        }
    }
}
