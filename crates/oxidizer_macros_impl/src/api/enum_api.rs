// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use proc_macro2::TokenStream;
use quote::quote;
use syn::spanned::Spanned;
use syn::{Attribute, ItemEnum, parse_quote};

#[expect(
    clippy::needless_pass_by_value,
    reason = "Convention for syn-based code"
)]
pub fn core_enum(attr: TokenStream, item: ItemEnum) -> super::Result<TokenStream> {
    if !attr.is_empty() {
        return Err(syn::Error::new(
            attr.span(),
            "The `oxidizer_api_lifecycle::api` attribute does not accept any arguments on enums.",
        ));
    }

    let non_exhaustive: Attribute = parse_quote! { #[non_exhaustive] };

    if item.attrs.contains(&non_exhaustive) {
        return Err(syn::Error::new(
            item.span(),
            "The `oxidizer_api_lifecycle::api` attribute automatically applies #[non_exhaustive] on enums - do not apply it manually.",
        ));
    }

    // TODO: We should also validate that enum variants are following all the rules, as each variant
    // is essentially its own little struct (either a value object or data transfer object).
    // This functionality is omitted in the current version, to be improved in later iterations.

    Ok(quote! {
        #non_exhaustive
        #item
    })
}

#[cfg(test)]
mod tests {
    use quote::quote;
    use syn::parse_quote;

    use super::*;

    #[test]
    fn smoke_test() {
        let input = parse_quote! {
            enum Something {
                First,
                Second,
            }
        };

        let result = core_enum(TokenStream::new(), input).unwrap();

        let expected = quote! {
            #[non_exhaustive]
            enum Something {
                First,
                Second,
            }
        };

        assert_eq!(result.to_string(), expected.to_string());
    }

    #[test]
    fn error_junk_arguments() {
        let attr = quote! { some, junk };

        let input = parse_quote! {
            enum Something {
                First,
                Second,
            }
        };

        core_enum(attr, input).unwrap_err();
    }

    #[test]
    fn error_if_non_exhaustive_already() {
        let input = parse_quote! {
            #[non_exhaustive]
            enum Something {
                First,
                Second,
            }
        };

        core_enum(TokenStream::new(), input).unwrap_err();
    }
}