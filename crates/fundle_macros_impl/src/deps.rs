// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use proc_macro2::{Ident, TokenStream};
use quote::quote;
use syn::{parse2, Data, DeriveInput, Fields, FieldsNamed, Type};

#[cfg_attr(test, mutants::skip)]
pub fn deps(_attr: TokenStream, item: TokenStream)  -> syn::Result<TokenStream>  {
    let input: DeriveInput = parse2(item)?;

    let struct_name = &input.ident;
    let struct_vis = &input.vis;
    let struct_attrs = &input.attrs;

    let fields = match &input.data {
        Data::Struct(data) => match &data.fields {
            Fields::Named(FieldsNamed { named, .. }) => {
                if named.is_empty() {
                    return Err(syn::Error::new_spanned(
                        struct_name,
                        "fundle::args requires at least one field",
                    ));
                }
                named
            }
            Fields::Unnamed(_) => {
                return Err(syn::Error::new_spanned(
                    struct_name,
                    "fundle::args only supports structs with named fields",
                ))
            }
            Fields::Unit => {
                return Err(syn::Error::new_spanned(
                    struct_name,
                    "fundle::args does not support unit structs",
                ))
            }
        },
        Data::Enum(_) | Data::Union(_) => {
            return Err(syn::Error::new_spanned(
                struct_name,
                "fundle::args can only be applied to structs",
            ));
        }
    };

    let field_types: Vec<_> = fields.iter().map(|f| &f.ty).collect();
    let field_names: Vec<_> = fields
        .iter()
        .map(|f| {
            f.ident.as_ref().expect(
                "internal error: named field without identifier (this should be impossible after validation)",
            )
        })
        .collect();

    let from_impl =
        generate_args_from_impl(struct_name, &field_names, &field_types, &input.generics);

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

    Ok(TokenStream::from(expanded))
}

#[cfg_attr(test, mutants::skip)]
fn generate_args_from_impl(
    struct_name: &Ident,
    field_names: &[&Ident],
    field_types: &[&Type],
    generics: &syn::Generics,
) -> proc_macro2::TokenStream {
    let as_ref_bounds = field_types.iter().map(|ty| quote!(AsRef<#ty>));

    let field_assignments = field_names
        .iter()
        .zip(field_types.iter())
        .map(|(name, ty)| quote!(#name: <T as AsRef<#ty>>::as_ref(&value).to_owned()));

    let (impl_generics, ty_generics, where_clause) = generics.split_for_impl();

    // Handle generics properly - add T to the impl generics
    let impl_generics_with_t = if generics.params.is_empty() {
        quote!(<T>)
    } else {
        quote!(<T, #impl_generics>)
    };

    // Handle where clause properly - if there are existing where clauses, add a comma
    let where_clause_tokens = if where_clause.is_some() {
        quote!(, #where_clause)
    } else {
        quote!()
    };

    quote! {
        #[allow(private_bounds)]
        impl #impl_generics_with_t From<T> for #struct_name #ty_generics
        where
            T: #(#as_ref_bounds)+*
            #where_clause_tokens
        {
            fn from(value: T) -> Self {
                Self {
                    #(#field_assignments),*
                }
            }
        }
    }
}
