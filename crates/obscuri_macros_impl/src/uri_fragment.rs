// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use proc_macro2::TokenStream;
use quote::quote;
use syn::{Data, DeriveInput, Fields};

use crate::bail;

/// Generates the `UriFragment` trait implementation for a newtype struct.
#[cfg_attr(test, mutants::skip)] // not relevant for auto-generated proc macros
pub(crate) fn uri_fragment_impl(input: DeriveInput) -> TokenStream {
    let ident = &input.ident;

    // Only support tuple structs (newtype pattern)
    let fields = match input.data {
        Data::Struct(ref data) => match data.fields {
            Fields::Unnamed(ref fields) => fields,
            _ => {
                bail!(input, "UriFragment can only be derived for tuple structs (newtype pattern)");
            }
        },
        Data::Enum(_) => {
            bail!(input, "UriFragment cannot be derived for enums");
        }
        Data::Union(_) => {
            bail!(input, "UriFragment cannot be derived for unions");
        }
    };

    // Ensure exactly one field
    let field_count = fields.unnamed.len();
    if field_count != 1 {
        bail!(fields, "UriFragment requires exactly one field, found {}", field_count);
    }

    // Generate the implementation
    quote! {
        impl ::obscuri::UriFragment for #ident {
            fn as_uri_safe(&self) -> impl ::obscuri::UriSafe {
                &self.0
            }
        }
    }
}

/// Generates the `UriFragment` trait implementation for a newtype struct.
pub(crate) fn uri_unsafe_fragment_impl(input: DeriveInput) -> TokenStream {
    let ident = &input.ident;

    // Only support tuple structs (newtype pattern)
    let fields = match input.data {
        Data::Struct(ref data) => match data.fields {
            Fields::Unnamed(ref fields) => fields,
            _ => {
                bail!(input, "UriUnsafeFragment can only be derived for tuple structs (newtype pattern)");
            }
        },
        Data::Enum(_) => {
            bail!(input, "UriUnsafeFragment cannot be derived for enums");
        }
        Data::Union(_) => {
            bail!(input, "UriUnsafeFragment cannot be derived for unions");
        }
    };

    // Ensure exactly one field
    let field_count = fields.unnamed.len();
    if field_count != 1 {
        bail!(fields, "UriUnsafeFragment requires exactly one field, found {}", field_count);
    }

    // Generate the implementation
    quote! {
        impl ::obscuri::UriUnsafeFragment for #ident {
            fn as_display(&self) -> impl ::std::fmt::Display {
                <Self as ::std::string::ToString>::to_string(self)
            }
        }
    }
}
