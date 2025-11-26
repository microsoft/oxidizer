// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use proc_macro2::TokenStream;
use quote::quote;
use syn::{DeriveInput, Result, parse2};

pub fn redacted_debug_impl(input: TokenStream) -> Result<TokenStream> {
    let input: DeriveInput = parse2(input)?;
    let name = &input.ident;

    todo!()
}

pub fn redacted_display_impl(input: TokenStream) -> Result<TokenStream> {
    let input: DeriveInput = parse2(input)?;
    let name = &input.ident;

    todo!()
}

pub fn redacted_to_string_impl(input: TokenStream) -> Result<TokenStream> {
    let input: DeriveInput = parse2(input)?;
    let name = &input.ident;

    todo!()
}


#[cfg(test)]
mod test {
    use crate::derive::*;
    use insta::assert_snapshot;
    use quote::quote;

    #[test]
    fn redacted_debug() {
        let input = quote! {
            struct EmailAddress(String);
        };

        let result = redacted_debug_impl(input);
        let result_file = syn::parse_file(&result.unwrap().to_string()).unwrap();
        let pretty = prettyplease::unparse(&result_file);

        assert_snapshot!(pretty);
    }

    #[test]
    fn redacted_display() {
        let input = quote! {
            struct EmailAddress(String);
        };

        let result = redacted_display_impl(input);
        let result_file = syn::parse_file(&result.unwrap().to_string()).unwrap();
        let pretty = prettyplease::unparse(&result_file);

        assert_snapshot!(pretty);
    }

    #[test]
    fn redacted_to_string() {
        let input = quote! {
            struct EmailAddress(String);
        };

        let result = redacted_to_string_impl(input);
        let result_file = syn::parse_file(&result.unwrap().to_string()).unwrap();
        let pretty = prettyplease::unparse(&result_file);

        assert_snapshot!(pretty);
    }

    #[test]
    fn classified_debug() {
        let input = quote! {
            struct EmailAddress(String);
        };

        let result = classified_debug_impl(input);
        let result_file = syn::parse_file(&result.unwrap().to_string()).unwrap();
        let pretty = prettyplease::unparse(&result_file);

        assert_snapshot!(pretty);
    }
}
