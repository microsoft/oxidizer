// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

// Main derive_error module structure
//
// This module implements the #[derive(Error)] macro functionality.
// It is split into logical submodules for better maintainability.

mod attributes;
mod constructors;
mod display;
mod field_detection;
mod from_impls;
#[cfg(test)]
mod tests;
pub mod types;

pub use attributes::*;
pub use constructors::*;
pub use display::*;
pub use field_detection::*;
pub use from_impls::*;
use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::{DeriveInput, Result, parse_macro_input};

/// Derive macro for automatically implementing error traits.
///
/// See the main `ohno` crate documentation for detailed usage examples.
#[cfg_attr(test, mutants::skip)] // procedural macro API cannot be used in tests directly
pub fn derive_error(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);

    impl_error_derive(&input).unwrap_or_else(|err| err.to_compile_error()).into()
}

fn impl_error_derive(input: &DeriveInput) -> Result<proc_macro2::TokenStream> {
    let name = &input.ident;
    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();

    // Parse attributes and find error field
    let error_field = find_error_field(input)?;
    let display_override = find_display_attribute(input)?;
    let display_expr = if let Some(display_override) = display_override {
        Some(parse_display_template(&display_override, input)?)
    } else {
        None
    };
    let from_configs = find_from_attribute(input)?;
    let should_impl_debug = !has_no_debug_attribute(input);

    // Generate implementations
    let constructor_impl = generate_constructor_impl(input, &error_field)?;
    let from_impl = generate_from_implementations(input, &error_field, &from_configs)?;
    let from_infallible_impl = generate_from_infallible_impl(name, &impl_generics, &ty_generics, where_clause);
    let display_impl = generate_display_impl(
        &error_field,
        display_expr.as_ref(),
        name,
        &impl_generics,
        &ty_generics,
        where_clause,
    );
    let error_impl = generate_error_impl(name, &impl_generics, &ty_generics, where_clause, &error_field);
    let enrichable_impl = generate_enrichable_impl(name, &impl_generics, &ty_generics, where_clause, &error_field);
    let error_ext_impl = generate_error_ext_impl(name, &impl_generics, &ty_generics, where_clause, &error_field, display_expr);
    let debug_impl = if should_impl_debug {
        generate_debug_impl(input, name, &impl_generics, &ty_generics, where_clause)
    } else {
        quote! {}
    };

    Ok(quote! {
        #constructor_impl
        #from_impl
        #from_infallible_impl
        #display_impl
        #error_impl
        #enrichable_impl
        #debug_impl
        #error_ext_impl
    })
}

// Helper functions for generating trait implementations

fn generate_constructor_impl(input: &DeriveInput, error_field: &types::ErrorFieldRef) -> Result<proc_macro2::TokenStream> {
    if has_no_constructors_attribute(input) {
        Ok(quote! {})
    } else {
        generate_constructor_methods(input, error_field)
    }
}

fn generate_from_infallible_impl(
    name: &syn::Ident,
    impl_generics: &syn::ImplGenerics,
    ty_generics: &syn::TypeGenerics,
    where_clause: Option<&syn::WhereClause>,
) -> proc_macro2::TokenStream {
    quote! {
        impl #impl_generics From<std::convert::Infallible> for #name #ty_generics #where_clause {
            fn from(_: std::convert::Infallible) -> Self {
                unreachable!("Infallible should never be converted to Error")
            }
        }
    }
}

#[expect(clippy::option_if_let_else, reason = "Would decrease readability")]
fn generate_display_impl(
    error_field: &types::ErrorFieldRef,
    display_expr: Option<&proc_macro2::TokenStream>,
    name: &syn::Ident,
    impl_generics: &syn::ImplGenerics,
    ty_generics: &syn::TypeGenerics,
    where_clause: Option<&syn::WhereClause>,
) -> proc_macro2::TokenStream {
    let error_field_access = error_field.to_field_access();

    if let Some(display_expr) = display_expr {
        quote! {
            impl #impl_generics std::fmt::Display for #name #ty_generics #where_clause {
                fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                    self.#error_field_access.format_error(f, stringify!(#name), Some(#display_expr))
                }
            }
        }
    } else {
        quote! {
            impl #impl_generics std::fmt::Display for #name #ty_generics #where_clause {
                fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                    self.#error_field_access.format_error(f, stringify!(#name), None)
                }
            }
        }
    }
}

fn generate_error_impl(
    name: &syn::Ident,
    impl_generics: &syn::ImplGenerics,
    ty_generics: &syn::TypeGenerics,
    where_clause: Option<&syn::WhereClause>,
    error_field: &types::ErrorFieldRef,
) -> proc_macro2::TokenStream {
    let error_field_access = error_field.to_field_access();
    quote! {
        impl #impl_generics std::error::Error for #name #ty_generics #where_clause {
            fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
                self.#error_field_access.source()
            }
        }
    }
}

fn generate_enrichable_impl(
    name: &syn::Ident,
    impl_generics: &syn::ImplGenerics,
    ty_generics: &syn::TypeGenerics,
    where_clause: Option<&syn::WhereClause>,
    error_field: &types::ErrorFieldRef,
) -> proc_macro2::TokenStream {
    let error_field_access = error_field.to_field_access();
    quote! {
        impl #impl_generics ohno::Enrichable for #name #ty_generics #where_clause {
            fn add_enrichment(&mut self, entry: ohno::EnrichmentEntry) {
                self.#error_field_access.add_enrichment(entry);
            }
        }
    }
}

#[expect(clippy::unwrap_used, reason = "Field names are guaranteed to be present here")]
fn generate_debug_impl(
    input: &DeriveInput,
    name: &syn::Ident,
    impl_generics: &syn::ImplGenerics,
    ty_generics: &syn::TypeGenerics,
    where_clause: Option<&syn::WhereClause>,
) -> proc_macro2::TokenStream {
    let debug_body = match &input.data {
        syn::Data::Struct(data_struct) => match &data_struct.fields {
            syn::Fields::Named(fields_named) => {
                let field_entries = fields_named.named.iter().map(|field| {
                    let field_name = field.ident.as_ref().unwrap();
                    let field_name_str = field_name.to_string();
                    quote! { .field(#field_name_str, &self.#field_name) }
                });
                quote! {
                    f.debug_struct(stringify!(#name))
                     #(#field_entries)*
                     .finish()
                }
            }
            syn::Fields::Unnamed(fields_unnamed) => {
                let field_entries = fields_unnamed.unnamed.iter().enumerate().map(|(i, _)| {
                    let field_index = syn::Index::from(i);
                    quote! { .field(&self.#field_index) }
                });
                quote! {
                    f.debug_tuple(stringify!(#name))
                     #(#field_entries)*
                     .finish()
                }
            }
            syn::Fields::Unit => {
                quote! { f.debug_struct(stringify!(#name)).finish() }
            }
        },
        _ => {
            // For enums, though we don't expect this case
            quote! { f.debug_struct(stringify!(#name)).finish_non_exhaustive() }
        }
    };

    quote! {
        impl #impl_generics ::core::fmt::Debug for #name #ty_generics #where_clause {
            fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                #debug_body
            }
        }
    }
}

fn generate_error_ext_impl(
    name: &syn::Ident,
    impl_generics: &syn::ImplGenerics,
    ty_generics: &syn::TypeGenerics,
    where_clause: Option<&syn::WhereClause>,
    error_field: &types::ErrorFieldRef,
    display_expr: Option<TokenStream2>,
) -> proc_macro2::TokenStream {
    let error_field_access = error_field.to_field_access();
    let message_expr = display_expr.map_or_else(|| quote! { None }, |expr| quote! { Some(#expr) });

    quote! {
        impl #impl_generics ohno::ErrorExt for #name #ty_generics #where_clause {
            fn message(&self) -> String {
                self.#error_field_access.format_message(stringify!(#name), #message_expr)
            }

            fn backtrace(&self) -> &std::backtrace::Backtrace {
                self.#error_field_access.backtrace()
            }
        }
    }
}
