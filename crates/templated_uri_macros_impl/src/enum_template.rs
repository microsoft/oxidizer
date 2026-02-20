// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use proc_macro2::{Ident, TokenStream};
use quote::quote;
use syn::{DataEnum, Fields};

use crate::bail;

#[expect(clippy::cognitive_complexity, reason = "Macros can be complex")]
pub fn enum_template(ident: &Ident, data: &DataEnum) -> TokenStream {
    let enum_name = ident.to_string();
    let mut all_variants = Vec::new();
    let mut variant_types = Vec::new();

    for variant in &data.variants {
        let Fields::Unnamed(fields) = &variant.fields else {
            bail!(
                variant,
                "Only unnamed fields (tuples) are supported in enum variants for TemplatedUri",
            )
        };
        if fields.unnamed.len() != 1 {
            bail!(
                fields,
                "TemplatedUri enum variants must have exactly one field containing a TemplatedUri struct",
            )
        }
        all_variants.push(&variant.ident);
        variant_types.push(&fields.unnamed[0].ty);
    }

    // Matching patterns for all the enum variants (`Foo::Variant(bar)` part of `Foo::Variant(bar) => {}`)
    let variant_matches: Vec<_> = all_variants
        .iter()
        .map(|variant_ident| {
            quote! { #ident::#variant_ident(template_variant) }
        })
        .collect();

    quote! {
        impl templated_uri::TemplatedPathAndQuery for #ident {
            fn rfc_6570_template(&self) -> &'static core::primitive::str {
                match self {
                    #(#variant_matches =>template_variant.rfc_6570_template()),*
                }
            }

            fn template(&self) -> &'static core::primitive::str {
                match self {
                    #(#variant_matches => template_variant.template()),*
                }
            }

            fn label(&self) -> ::core::option::Option<&'static core::primitive::str> {
                match self {
                    #(#variant_matches => template_variant.label()),*
                }
            }

            fn to_uri_string(&self) -> ::std::string::String {
                match self {
                    #(#variant_matches => template_variant.to_uri_string()),*
                }
            }

            fn to_path_and_query(&self) -> ::std::result::Result<templated_uri::uri::PathAndQuery, templated_uri::ValidationError> {
                match self {
                    #(#variant_matches => template_variant.to_path_and_query()),*
                }
            }
        }

        impl ::std::fmt::Debug for #ident {
            fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
                match self {
                    #(#variant_matches => f.debug_tuple(#enum_name).field(&template_variant).finish()),*
                }
            }
        }

        impl ::data_privacy::RedactedDisplay for #ident {
            fn fmt(&self, engine: &::data_privacy::RedactionEngine, f: &mut ::std::fmt::Formatter) -> ::std::fmt::Result {
                match self {
                    #(#variant_matches => template_variant.fmt(engine, f)?),*
                }
                Ok(())
            }
        }

        #(
            impl ::std::convert::From<#variant_types> for #ident {
                fn from(template_variant: #variant_types) -> Self {
                    Self::#all_variants(template_variant)
                }
            }
        )*

        impl From<#ident> for templated_uri::uri::TargetPathAndQuery {
            fn from(value: #ident) -> Self {
                templated_uri::uri::TargetPathAndQuery::TemplatedPathAndQuery(std::sync::Arc::new(value))
            }
        }
    }
}
