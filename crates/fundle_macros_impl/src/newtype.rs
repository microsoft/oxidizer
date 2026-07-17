// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use proc_macro2::TokenStream;
use quote::quote;
use syn::{Fields, ItemStruct, parse2};

/// Attribute macro to create a newtype wrapper around a single inner type.
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

    let from_param = quote::format_ident!("__FundleFromT");
    let impl_generics_with_from = if generics.params.is_empty() {
        quote!(<#from_param>)
    } else {
        let params = &generics.params;
        quote!(<#params, #from_param>)
    };
    let additional_predicates = where_clause.map_or_else(
        || quote!(),
        |wc| {
            let predicates = &wc.predicates;
            quote!(, #predicates)
        },
    );

    let expanded = quote! {
        #[derive(::std::clone::Clone)]
        #[allow(dead_code)]
        #vis struct #name #generics(#inner_type) #where_clause;

        impl #impl_generics_with_from ::std::convert::From<#from_param> for #name #ty_generics
        where
            #from_param: ::std::convert::AsRef<#inner_type>,
            #inner_type: ::std::clone::Clone
            #additional_predicates
        {
            fn from(x: #from_param) -> Self {
                Self(x.as_ref().clone())
            }
        }

        impl #impl_generics ::std::ops::Deref for #name #ty_generics #where_clause {
            type Target = #inner_type;

            fn deref(&self) -> &Self::Target {
                &self.0
            }
        }

        impl #impl_generics ::std::ops::DerefMut for #name #ty_generics #where_clause {
            fn deref_mut(&mut self) -> &mut Self::Target {
                &mut self.0
            }
        }
    };

    Ok(expanded)
}

#[cfg(test)]
mod tests {
    use quote::quote;

    use super::newtype;

    #[test]
    fn rejects_named_struct() {
        let err = newtype(quote! {}, quote! { struct Named { inner: u32 } }).expect_err("named struct is not a tuple struct");
        assert!(err.to_string().contains("tuple structs"));
    }

    #[test]
    fn rejects_multiple_fields() {
        let err = newtype(quote! {}, quote! { struct Two(u32, u32); }).expect_err("multi-field tuple struct is rejected");
        assert!(err.to_string().contains("tuple structs"));
    }
}
