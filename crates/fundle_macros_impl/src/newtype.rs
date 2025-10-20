// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use proc_macro2::TokenStream;
use quote::quote;
use syn::{Fields, ItemStruct, parse2};

pub fn newtype(_attr: TokenStream, item: TokenStream) -> syn::Result<TokenStream> {
    let input: ItemStruct = parse2(item)?;
    let name = &input.ident;
    let vis = &input.vis;
    let generics = &input.generics;
    let (impl_generics, ty_generics, where_clause) = generics.split_for_impl();

    // Extract the inner type from the tuple struct
    let Fields::Unnamed(fields) = &input.fields else {
        return Err(syn::Error::new_spanned(
            &input,
            "fundle::newtype can only be applied to tuple structs with exactly one field",
        ));
    };

    if fields.unnamed.len() != 1 {
        return Err(syn::Error::new_spanned(
            &input,
            "fundle::newtype can only be applied to tuple structs with exactly one field",
        ));
    }

    let inner_type = &fields
        .unnamed
        .first()
        .expect("internal error: validated len == 1 but first() is None")
        .ty;

    let expanded = quote! {
        #[derive(Clone)]
        #[allow(dead_code)]
        #vis struct #name #generics(#inner_type) #where_clause;

        impl<T> #impl_generics From<T> for #name #ty_generics
        where
            T: AsRef<#inner_type>,
            #inner_type: Clone,
            #where_clause
        {
            fn from(x: T) -> Self {
                Self(x.as_ref().clone())
            }
        }

        impl #impl_generics std::ops::Deref for #name #ty_generics #where_clause {
            type Target = #inner_type;

            fn deref(&self) -> &Self::Target {
                &self.0
            }
        }

        impl #impl_generics std::ops::DerefMut for #name #ty_generics #where_clause {
            fn deref_mut(&mut self) -> &mut Self::Target {
                &mut self.0
            }
        }
    };

    Ok(expanded)
}
