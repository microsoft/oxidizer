// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use proc_macro::TokenStream;
use quote::quote;
use syn::{Data, DeriveInput, Fields, parse_macro_input};

#[cfg_attr(test, mutants::skip)]
pub fn newtype(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let input = parse_macro_input!(item as DeriveInput);
    let name = &input.ident;
    let vis = &input.vis;
    let generics = &input.generics;
    let (impl_generics, ty_generics, where_clause) = generics.split_for_impl();

    // Extract the inner type from the tuple struct
    let inner_type = match &input.data {
        Data::Struct(data_struct) => match &data_struct.fields {
            Fields::Unnamed(fields) if fields.unnamed.len() == 1 => {
                &fields.unnamed.first().unwrap().ty
            }
            _ => {
                return syn::Error::new_spanned(
                    &input,
                    "fundle::newtype can only be applied to tuple structs with exactly one field",
                )
                .to_compile_error()
                .into();
            }
        },
        _ => {
            return syn::Error::new_spanned(
                &input,
                "fundle::newtype can only be applied to structs",
            )
            .to_compile_error()
            .into();
        }
    };

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

    TokenStream::from(expanded)
}
