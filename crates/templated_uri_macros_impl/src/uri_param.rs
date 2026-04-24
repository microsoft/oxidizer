// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use proc_macro2::TokenStream;
use quote::quote;
use syn::{Data, DeriveInput, Fields};

use crate::bail;

/// Generates the `Escape` trait implementation for a newtype struct.
#[cfg_attr(test, mutants::skip)] // not relevant for auto-generated proc macros
pub(crate) fn uri_param_impl(input: DeriveInput) -> TokenStream {
    let ident = &input.ident;

    if !input.generics.params.is_empty() {
        return syn::Error::new_spanned(&input.generics, "Escape cannot be derived for generic types").to_compile_error();
    }

    // Only support tuple structs (newtype pattern)
    let fields = match input.data {
        Data::Struct(ref data) => match data.fields {
            Fields::Unnamed(ref fields) => fields,
            _ => {
                bail!(input, "Escape can only be derived for tuple structs (newtype pattern)");
            }
        },
        Data::Enum(_) => {
            bail!(input, "Escape cannot be derived for enums");
        }
        Data::Union(_) => {
            bail!(input, "Escape cannot be derived for unions");
        }
    };

    // Ensure exactly one field
    let field_count = fields.unnamed.len();
    if field_count != 1 {
        bail!(fields, "Escape requires exactly one field, found {}", field_count);
    }

    // Generate the implementation
    quote! {
        impl ::templated_uri::Escape for #ident {
            fn escape(&self) -> ::templated_uri::Escaped<impl ::std::fmt::Display> {
                self.0.escape()
            }
        }
    }
}

/// Generates the `UnescapedDisplay` trait implementation for a newtype struct.
pub(crate) fn uri_unsafe_param_impl(input: DeriveInput) -> TokenStream {
    let ident = &input.ident;

    if !input.generics.params.is_empty() {
        return syn::Error::new_spanned(&input.generics, "UnescapedDisplay cannot be derived for generic types").to_compile_error();
    }

    // Only support tuple structs (newtype pattern)
    let fields = match input.data {
        Data::Struct(ref data) => match data.fields {
            Fields::Unnamed(ref fields) => fields,
            _ => {
                bail!(input, "UnescapedDisplay can only be derived for tuple structs (newtype pattern)");
            }
        },
        Data::Enum(_) => {
            bail!(input, "UnescapedDisplay cannot be derived for enums");
        }
        Data::Union(_) => {
            bail!(input, "UnescapedDisplay cannot be derived for unions");
        }
    };

    // Ensure exactly one field
    let field_count = fields.unnamed.len();
    if field_count != 1 {
        bail!(fields, "UnescapedDisplay requires exactly one field, found {}", field_count);
    }

    // Generate the implementation
    quote! {
        impl ::templated_uri::UnescapedDisplay for #ident {
            fn unescaped_display(&self) -> impl ::std::fmt::Display {
                &self.0
            }
        }
    }
}
