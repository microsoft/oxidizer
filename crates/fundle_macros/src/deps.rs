// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use proc_macro2::{Ident, TokenStream};
use quote::quote;
use syn::{Fields, FieldsNamed, ItemStruct, Type, parse2};

pub fn deps_impl(_attr: TokenStream, item: TokenStream) -> syn::Result<TokenStream> {
    let input: ItemStruct = parse2(item)?;

    let struct_name = &input.ident;
    let struct_vis = &input.vis;
    let struct_attrs = &input.attrs;

    let Fields::Named(FieldsNamed { named: fields, .. }) = &input.fields else {
        return Err(syn::Error::new_spanned(
            struct_name,
            "fundle::deps only supports structs with named fields",
        ));
    };

    if fields.is_empty() {
        return Err(syn::Error::new_spanned(struct_name, "fundle::deps requires at least one field"));
    }

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
        .map(|(name, ty)| quote!(#name: <#from_param as ::std::convert::AsRef<#ty>>::as_ref(&value).to_owned()));

    let (_impl_generics, ty_generics, where_clause) = generics.split_for_impl();

    // Add the From parameter to the generics
    let impl_generics_with_from = if generics.params.is_empty() {
        quote!(<#from_param>)
    } else {
        let params = &generics.params;
        quote!(<#params, #from_param>)
    };

    let as_ref_bounds = field_types.iter().map(|ty| quote!(::std::convert::AsRef<#ty>));

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
        impl #impl_generics_with_from ::std::convert::From<#from_param> for #struct_name #ty_generics
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

#[cfg(test)]
mod tests {
    use syn::{ItemStruct, ItemUnion, parse_quote};

    macro_rules! expand_fundle_deps {
        ($item:expr) => {{
            // Extract just the arguments from inside the attribute (empty in this case)
            let attr_args = if let Some(attr) = $item.attrs.iter().find(|a| a.path().is_ident("deps")) {
                match &attr.meta {
                    syn::Meta::Path(_) => quote::quote! {},       // #[deps] with no args
                    syn::Meta::List(list) => list.tokens.clone(), // #[deps(...)]
                    syn::Meta::NameValue(_) => quote::quote! {},  // shouldn't happen for your use case
                }
            } else {
                quote::quote! {}
            };

            // Create a clean item without the deps attribute
            let mut clean_item = $item.clone();
            clean_item.attrs.retain(|attr| !attr.path().is_ident("deps"));
            let item_tokens = quote::quote! { #clean_item };

            let output = crate::deps::deps_impl(attr_args, item_tokens).unwrap_or_else(|e| e.to_compile_error());

            // Parse as File - the output should be a complete set of items
            let file: syn::File = syn::parse2(output).unwrap();
            prettyplease::unparse(&file)
        }};
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn basic_expansion() {
        let item: ItemStruct = parse_quote! {
            #[deps]
            struct Foo {
                x: String
            }
        };

        insta::assert_snapshot!(expand_fundle_deps!(item));
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn empty() {
        let item: ItemStruct = parse_quote! {
            #[deps]
            struct Foo {}
        };

        insta::assert_snapshot!(expand_fundle_deps!(item));
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn tuple() {
        let item: ItemStruct = parse_quote! {
            #[deps]
            struct Foo(u8);
        };

        insta::assert_snapshot!(expand_fundle_deps!(item));
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn unit() {
        let item: ItemStruct = parse_quote! {
            #[deps]
            struct Foo;
        };

        insta::assert_snapshot!(expand_fundle_deps!(item));
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn union() {
        let item: ItemUnion = parse_quote! {
            #[deps]
            union Foo {}
        };

        insta::assert_snapshot!(expand_fundle_deps!(item));
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn lifetimes() {
        let item: ItemStruct = parse_quote! {
            #[deps]
            struct Foo<'a> {
                x: &'a u32
            }
        };

        insta::assert_snapshot!(expand_fundle_deps!(item));
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn generics() {
        let item: ItemStruct = parse_quote! {
            #[deps]
            struct Foo<T> {
                x: T
            }
        };

        insta::assert_snapshot!(expand_fundle_deps!(item));
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn generics_where() {
        let item: ItemStruct = parse_quote! {
            #[deps]
            struct Foo<T> where T: Clone {
                x: T
            }
        };

        insta::assert_snapshot!(expand_fundle_deps!(item));
    }
}
