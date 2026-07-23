// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use alloc::string::ToString as _;

use syn::parse::{Parse, ParseStream};
use syn::{Ident, Token};

#[derive(Default)]
#[cfg(feature = "resolve")]
pub(super) struct ResolverAttr {
    pub(super) name: Option<Ident>,
}

#[cfg(feature = "resolve")]
impl Parse for ResolverAttr {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        if input.is_empty() {
            return Ok(Self::default());
        }

        let key: Ident = input.parse()?;
        if key != "name" {
            return Err(syn::Error::new(key.span(), "expected `name = ResolverType`"));
        }
        let _equals: Token![=] = input.parse()?;
        let name: Ident = input.parse()?;
        if name.to_string().starts_with("r#") {
            return Err(syn::Error::new(name.span(), "the resolver type name cannot be a raw identifier"));
        }
        let _trailing_comma = input.parse::<Option<Token![,]>>()?;
        if !input.is_empty() {
            return Err(input.error("unexpected resolver attribute argument"));
        }
        Ok(Self { name: Some(name) })
    }
}

#[cfg(all(test, feature = "resolve"))]
mod tests {
    use quote::quote;

    use super::*;

    #[test]
    #[cfg(feature = "resolve")]
    fn resolver_accepts_no_arguments() {
        let attr = syn::parse2::<ResolverAttr>(quote! {}).expect("`#[resolver]` (bare) is accepted");
        assert!(attr.name.is_none());
    }

    #[test]
    #[cfg(feature = "resolve")]
    fn resolver_accepts_an_explicit_name_and_rejects_other_arguments() {
        let attr = syn::parse2::<ResolverAttr>(quote! { name = ApiResolver }).expect("explicit resolver name is accepted");
        assert_eq!(attr.name.expect("name is present"), "ApiResolver");
        let attr = syn::parse2::<ResolverAttr>(quote! { name = ApiResolver, }).expect("a trailing comma is accepted");
        assert_eq!(attr.name.expect("name is present"), "ApiResolver");

        for invalid in [
            quote! { ApiResolver },
            quote! { type_name = ApiResolver },
            quote! { name = r#type },
            quote! { name = ApiResolver, extra },
        ] {
            assert!(syn::parse2::<ResolverAttr>(invalid).is_err());
        }
    }
}
