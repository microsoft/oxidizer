// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use proc_macro2::TokenStream;
use syn::spanned::Spanned;
use syn::{Error, Item};

use self::enum_api::core_enum;
use self::struct_api::core_struct;
use crate::syn_helpers::token_stream_and_error;

mod enum_api;
mod options;
mod struct_api;

pub type Result<T> = std::result::Result<T, Error>;

#[must_use]
pub fn entrypoint(attr: TokenStream, input: TokenStream) -> TokenStream {
    let item_ast = syn::parse2::<Item>(input.clone());

    let result = match item_ast {
        Ok(Item::Enum(item)) => core_enum(attr, item),
        Ok(Item::Struct(item)) => core_struct(attr, item),
        Ok(x) => Err(syn::Error::new(
            x.span(),
            "The `oxidizer_api_lifecycle::api` attribute must be applied to a struct or enum.",
        )),
        Err(e) => Err(e),
    };

    match result {
        Ok(r) => r,
        Err(e) => token_stream_and_error(input, e),
    }
}

#[cfg(test)]
mod tests {
    use quote::quote;

    use super::*;
    use crate::syn_helpers::contains_compile_error;

    #[test]
    fn enum_smoke_test() {
        let input = quote! {
            enum Something {
                First,
                Second,
            }
        };

        let result = entrypoint(TokenStream::new(), input);
        assert!(!contains_compile_error(&result));
    }

    #[test]
    fn struct_smoke_test() {
        let input = quote! {
            /// Yolo wowza.
            pub struct Foo {
                /// This is a doc comment.
                length: usize,

                #[field(optional)]
                timeout_seconds: usize,

                name: Option<String>,
            }
        };

        let meta = quote! { dto, config };

        let result = entrypoint(meta, input);
        assert!(!contains_compile_error(&result));
    }

    #[test]
    fn error_on_trait() {
        let input = quote! {
            pub trait Foo {
            }
        };

        let result = entrypoint(TokenStream::new(), input);
        assert!(contains_compile_error(&result));
    }

    #[test]
    fn error_on_nonsense() {
        let input = quote! {
            ,,,,,,,,,,,,,,
        };

        let result = entrypoint(TokenStream::new(), input);
        assert!(contains_compile_error(&result));
    }
}