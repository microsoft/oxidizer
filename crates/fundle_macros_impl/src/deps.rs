// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use proc_macro2::{Ident, TokenStream};
use quote::quote;
use syn::{Fields, FieldsNamed, ItemStruct, Type, parse2};

pub fn deps(_attr: TokenStream, item: TokenStream) -> syn::Result<TokenStream> {
    let input: ItemStruct = parse2(item)?;

    let struct_name = &input.ident;
    let struct_vis = &input.vis;
    let struct_attrs = &input.attrs;

    let fields = match &input.fields {
        Fields::Named(FieldsNamed { named, .. }) => {
            if named.is_empty() {
                return Err(syn::Error::new_spanned(struct_name, "fundle::deps requires at least one field"));
            }
            named
        }
        Fields::Unnamed(_) => {
            return Err(syn::Error::new_spanned(
                struct_name,
                "fundle::deps only supports structs with named fields",
            ));
        }
        Fields::Unit => {
            return Err(syn::Error::new_spanned(struct_name, "fundle::deps does not support unit structs"));
        }
    };

    let field_types: Vec<_> = fields.iter().map(|f| &f.ty).collect();
    let field_names: Vec<_> = fields
        .iter()
        .map(|f| {
            f.ident
                .as_ref()
                .expect("internal error: named field without identifier (this should be impossible after validation)")
        })
        .collect();

    let from_impl = generate_args_from_impl(struct_name, &field_names, &field_types, &input.generics);

    // Split generics for proper struct definition
    let (_impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();

    let expanded = quote! {
        #(#struct_attrs)*
        #[allow(private_bounds)]
        #struct_vis struct #struct_name #ty_generics #where_clause {
            #fields
        }

        #from_impl
    };

    Ok(expanded)
}

#[cfg_attr(test, mutants::skip)]
fn generate_args_from_impl(
    struct_name: &Ident,
    field_names: &[&Ident],
    field_types: &[&Type],
    generics: &syn::Generics,
) -> proc_macro2::TokenStream {
    // Use a unique name for the From<T> parameter to avoid conflicts
    // We'll use __FundleFromT as it's unlikely to conflict with user types
    let from_param = quote::format_ident!("__FundleFromT");

    let field_assignments = field_names
        .iter()
        .zip(field_types.iter())
        .map(|(name, ty)| quote!(#name: <#from_param as AsRef<#ty>>::as_ref(&value).to_owned()));

    let (_impl_generics, ty_generics, where_clause) = generics.split_for_impl();

    // Add the From parameter to the generics
    let impl_generics_with_from = if generics.params.is_empty() {
        quote!(<#from_param>)
    } else {
        let params = &generics.params;
        quote!(<#params, #from_param>)
    };

    let as_ref_bounds = field_types.iter().map(|ty| quote!(AsRef<#ty>));

    // Handle where clause properly - extract just the predicates without the 'where' keyword
    let additional_predicates = where_clause.map_or_else(
        || quote!(),
        |wc| {
            let predicates = &wc.predicates;
            quote!(, #predicates)
        },
    );

    quote! {
        #[allow(private_bounds)]
        impl #impl_generics_with_from From<#from_param> for #struct_name #ty_generics
        where
            #from_param: #(#as_ref_bounds)+*
            #additional_predicates
        {
            fn from(value: #from_param) -> Self {
                Self {
                    #(#field_assignments),*
                }
            }
        }
    }
}
