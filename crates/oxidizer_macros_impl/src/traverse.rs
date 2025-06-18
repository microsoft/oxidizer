// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::ItemStruct;
use syn::spanned::Spanned;

use crate::syn_helpers::token_stream_and_error;

#[must_use]
pub fn entrypoint(input: TokenStream) -> TokenStream {
    match core(input.clone()) {
        Ok(result) => result,
        Err(e) => token_stream_and_error(input, e),
    }
}

fn core(input: TokenStream) -> Result<TokenStream, syn::Error> {
    let ast = syn::parse2::<ItemStruct>(input)?;

    let name = &ast.ident;
    let trait_name = format_ident!("Traverse{}", name);

    let mut trait_methods = Vec::new();
    let mut impl_methods = Vec::new();

    if let syn::Fields::Named(syn::FieldsNamed { named: fields, .. }) = &ast.fields {
        for field in fields {
            let field_name = field.ident.as_ref().unwrap();
            let field_name_str = field_name.to_string();
            let field_type = &field.ty;

            trait_methods.push(quote! {
                fn #field_name(&self) -> ::oxidizer_config::View<#field_type>;
            });

            impl_methods.push(quote! {
                fn #field_name(&self) -> ::oxidizer_config::View<#field_type> {
                    self.map(|value| &value.#field_name, #field_name_str)
                }
            });
        }
    } else {
        return Err(syn::Error::new(
            ast.span(),
            "You can only use the traverse macro on structs with named fields.",
        ));
    }

    let (impl_generics, ty_generics, where_clause) = ast.generics.split_for_impl();

    let trait_def = quote! {
        pub trait #trait_name #impl_generics #where_clause {
            #(#trait_methods)*
        }
    };

    let impl_def = quote! {
        impl #impl_generics #trait_name #ty_generics
            for ::oxidizer_config::View<#name #ty_generics>
            #where_clause
        {
            #(#impl_methods)*
        }
    };

    let expanded = quote! {
        #trait_def
        #impl_def
        #ast
    };

    Ok(expanded)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syn_helpers::contains_compile_error;

    #[test]
    fn ok_on_structs_with_named_fields() {
        let input = quote! {
            struct Foo {
                bar: usize,
            }
        };

        let result = entrypoint(input);
        assert!(!contains_compile_error(&result));
    }

    #[test]
    fn nok_on_structs_with_unnamed_fields() {
        let input = quote! {
            struct Foo(usize);
        };

        let result = entrypoint(input);
        assert!(contains_compile_error(&result));
    }

    #[test]
    fn nok_on_nonstructs() {
        let input = quote! {
            enum Foo {
                Bar,
                Baz,
            }
        };

        let result = entrypoint(input);
        assert!(contains_compile_error(&result));
    }
}