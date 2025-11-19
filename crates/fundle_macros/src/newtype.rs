// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use proc_macro2::TokenStream;
use quote::quote;
use syn::{Fields, ItemStruct, parse2};

pub fn newtype_impl(_attr: TokenStream, item: TokenStream) -> syn::Result<TokenStream> {
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
        #[derive(::std::clone::Clone)]
        #[allow(dead_code)]
        #vis struct #name #generics(#inner_type) #where_clause;

        impl<T> #impl_generics ::std::convert::From<T> for #name #ty_generics
        where
            T: ::std::convert::AsRef<#inner_type>,
            #inner_type: ::std::clone::Clone,
            #where_clause
        {
            fn from(x: T) -> Self {
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
    use syn::{ItemStruct, ItemUnion, parse_quote};

    macro_rules! expand_fundle_newtype {
        ($item:expr) => {{
            // Extract just the arguments from inside the attribute (empty in this case)
            let attr_args = if let Some(attr) = $item.attrs.iter().find(|a| a.path().is_ident("newtype")) {
                match &attr.meta {
                    syn::Meta::Path(_) => quote::quote! {},       // #[newtype] with no args
                    syn::Meta::List(list) => list.tokens.clone(), // #[newtype(...)]
                    syn::Meta::NameValue(_) => quote::quote! {},  // shouldn't happen for your use case
                }
            } else {
                quote::quote! {}
            };

            // Create a clean item without the newtype attribute
            let mut clean_item = $item.clone();
            clean_item.attrs.retain(|attr| !attr.path().is_ident("newtype"));
            let item_tokens = quote::quote! { #clean_item };

            let output = crate::newtype::newtype_impl(attr_args, item_tokens).unwrap_or_else(|e| e.to_compile_error());

            // Parse as File - the output should be a complete set of items
            let file: syn::File = syn::parse2(output).unwrap();
            prettyplease::unparse(&file)
        }};
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn basic_expansion() {
        let item: ItemStruct = parse_quote! {
            #[newtype]
            struct Foo(String);
        };

        insta::assert_snapshot!(expand_fundle_newtype!(item));
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn empty_structs() {
        let item: ItemStruct = parse_quote! {
            #[newtype]
            struct Foo;
        };

        insta::assert_snapshot!(expand_fundle_newtype!(item));
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn multi_field() {
        let item: ItemStruct = parse_quote! {
            #[newtype]
            struct Foo(String, String);
        };

        insta::assert_snapshot!(expand_fundle_newtype!(item));
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn multi_field_named() {
        let item: ItemStruct = parse_quote! {
            #[newtype]
            struct Foo {
                x: String,
                y: String
            }
        };

        insta::assert_snapshot!(expand_fundle_newtype!(item));
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn union() {
        let item: ItemUnion = parse_quote! {
            #[newtype]
            union Foo {
                x: String,
                y: String
            }
        };

        insta::assert_snapshot!(expand_fundle_newtype!(item));
    }
}
