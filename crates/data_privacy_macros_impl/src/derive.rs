// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Macros supporting various derives.

use proc_macro2::TokenStream;
use quote::quote;
use syn::{DeriveInput, Result, parse2};

/// Check if a field has the `#[unredacted]` attribute
fn is_unredacted(field: &syn::Field) -> bool {
    field.attrs.iter().any(|attr| attr.path().is_ident("unredacted"))
}

/// Derive the `RedactedDebug` trait for a struct.
pub fn redacted_debug(input: TokenStream) -> Result<TokenStream> {
    let input: DeriveInput = parse2(input)?;
    let name = &input.ident;
    let generics = &input.generics;
    let (impl_generics, ty_generics, where_clause) = generics.split_for_impl();

    let syn::Data::Struct(data_struct) = &input.data else {
        return Err(syn::Error::new_spanned(input, "RedactedDebug can only be derived for structs"));
    };

    let field_fmt_calls = match &data_struct.fields {
        syn::Fields::Named(fields) => {
            let calls = fields.named.iter().enumerate().map(|(i, field)| {
                let field_name = &field.ident;
                let field_name_str = field_name.as_ref().unwrap().to_string();
                let field_type = &field.ty;
                let unredacted = is_unredacted(field);

                let separator = if i == 0 { " " } else { ", " };

                if unredacted {
                    quote! {
                        ::std::write!(f, "{}{}: {:?}", #separator, #field_name_str, &self.#field_name)?;
                    }
                } else {
                    quote! {
                        ::std::write!(f, "{}{}: ", #separator, #field_name_str)?;
                        <#field_type as ::data_privacy::RedactedDebug>::fmt(&self.#field_name, engine, f)?;
                    }
                }
            });
            quote! { #(#calls)* }
        }
        syn::Fields::Unnamed(fields) => {
            let calls = fields.unnamed.iter().enumerate().map(|(i, field)| {
                let field_type = &field.ty;
                let index = syn::Index::from(i);
                let unredacted = is_unredacted(field);

                let separator = if i > 0 {
                    quote! { ::std::write!(f, ", ")?; }
                } else {
                    quote! {}
                };

                let format_call = if unredacted {
                    quote! {
                        ::std::write!(f, "{:?}", &self.#index)?;
                    }
                } else {
                    quote! {
                        <#field_type as ::data_privacy::RedactedDebug>::fmt(&self.#index, engine, f)?;
                    }
                };

                quote! {
                    #separator
                    #format_call
                }
            });
            quote! { #(#calls)* }
        }
        syn::Fields::Unit => {
            quote! {}
        }
    };

    let name_str = name.to_string();
    let (opening, closing) = match &data_struct.fields {
        syn::Fields::Named(_) => (format!("{name_str} {{{{"), " }}"),
        syn::Fields::Unnamed(_) => (format!("{name_str}("), ")"),
        syn::Fields::Unit => (name_str, ""),
    };

    Ok(quote! {
        impl #impl_generics ::data_privacy::RedactedDebug for #name #ty_generics #where_clause {
            fn fmt(
                &self,
                engine: &::data_privacy::RedactionEngine,
                f: &mut ::std::fmt::Formatter,
            ) -> ::std::fmt::Result {
                ::std::write!(f, #opening)?;
                #field_fmt_calls
                ::std::write!(f, #closing)?;
                ::std::result::Result::Ok(())
            }
        }
    })
}

/// Derive the `RedactedDisplay` trait for a struct.
pub fn redacted_display(input: TokenStream) -> Result<TokenStream> {
    let input: DeriveInput = parse2(input)?;
    let name = &input.ident;
    let generics = &input.generics;
    let (impl_generics, ty_generics, where_clause) = generics.split_for_impl();

    let syn::Data::Struct(data_struct) = &input.data else {
        return Err(syn::Error::new_spanned(input, "RedactedDisplay can only be derived for structs"));
    };

    let field_fmt_calls = match &data_struct.fields {
        syn::Fields::Named(fields) => {
            let calls = fields.named.iter().enumerate().map(|(i, field)| {
                let field_name = &field.ident;
                let field_name_str = field_name.as_ref().unwrap().to_string();
                let field_type = &field.ty;
                let unredacted = is_unredacted(field);

                let separator = if i == 0 { " " } else { ", " };

                if unredacted {
                    quote! {
                        ::std::write!(f, "{}{}: {}", #separator, #field_name_str, &self.#field_name)?;
                    }
                } else {
                    quote! {
                        ::std::write!(f, "{}{}: ", #separator, #field_name_str)?;
                        <#field_type as ::data_privacy::RedactedDisplay>::fmt(&self.#field_name, engine, f)?;
                    }
                }
            });
            quote! { #(#calls)* }
        }
        syn::Fields::Unnamed(fields) => {
            let calls = fields.unnamed.iter().enumerate().map(|(i, field)| {
                let field_type = &field.ty;
                let index = syn::Index::from(i);
                let unredacted = is_unredacted(field);

                let separator = if i > 0 {
                    quote! { ::std::write!(f, ", ")?; }
                } else {
                    quote! {}
                };

                let format_call = if unredacted {
                    quote! {
                        ::std::write!(f, "{}", &self.#index)?;
                    }
                } else {
                    quote! {
                        <#field_type as ::data_privacy::RedactedDisplay>::fmt(&self.#index, engine, f)?;
                    }
                };

                quote! {
                    #separator
                    #format_call
                }
            });
            quote! { #(#calls)* }
        }
        syn::Fields::Unit => {
            quote! {}
        }
    };

    let name_str = name.to_string();
    let (opening, closing) = match &data_struct.fields {
        syn::Fields::Named(_) => (format!("{name_str} {{{{"), " }}"),
        syn::Fields::Unnamed(_) => (format!("{name_str}("), ")"),
        syn::Fields::Unit => (name_str, ""),
    };

    Ok(quote! {
        impl #impl_generics ::data_privacy::RedactedDisplay for #name #ty_generics #where_clause {
            fn fmt(
                &self,
                engine: &::data_privacy::RedactionEngine,
                f: &mut ::std::fmt::Formatter,
            ) -> ::std::fmt::Result {
                ::std::write!(f, #opening)?;
                #field_fmt_calls
                ::std::write!(f, #closing)?;
                ::std::result::Result::Ok(())
            }
        }
    })
}
