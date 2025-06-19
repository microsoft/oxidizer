// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use proc_macro2::TokenStream;
use quote::quote;
use syn::DeriveInput;
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
    let ast = syn::parse2::<DeriveInput>(input)?;

    let name = &ast.ident;

    let mut keys = Vec::new();
    let mut gets = Vec::new();

    if let syn::Data::Struct(syn::DataStruct {
        fields: syn::Fields::Named(syn::FieldsNamed { named: fields, .. }),
        ..
    }) = &ast.data
    {
        for field in fields {
            let field_name = field.ident.as_ref().unwrap();
            let field_name_str = field_name.to_string();

            keys.push(quote! { keys.insert(#field_name_str); });
            gets.push(quote! {
                #field_name_str => Some(::std::convert::Into::<String>::into(&self.#field_name)),
            });
        }
    } else {
        return Err(syn::Error::new(
            ast.span(),
            "You can only derive a Context impl for structs with named fields.",
        ));
    }

    let expanded = quote! {
        impl ::oxidizer_config::Context for #name {
            fn keys() -> ::std::collections::HashSet<&'static str> {
                let mut keys = ::std::collections::HashSet::new();
                #(#keys)*
                keys
            }

            fn get(&self, key: &str) -> ::std::option::Option<::std::string::String> {
                match key {
                    #(#gets)*
                    _ => None,
                }
            }
        }
    };

    Ok(expanded)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syn_helpers::contains_compile_error;

    #[test]
    fn ok_on_structs() {
        let input = quote! {
            struct Foo {
                bar: usize,
            }
        };

        let result = entrypoint(input);
        assert!(!contains_compile_error(&result));
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