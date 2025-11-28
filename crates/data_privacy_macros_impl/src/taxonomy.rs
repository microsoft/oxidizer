// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use proc_macro2::{Ident, TokenStream};
use quote::quote;
use syn::parse::Parse;
use syn::spanned::Spanned;
use syn::{Fields, ItemEnum, parse2};

type SynResult<T> = Result<T, syn::Error>;

struct MacroArgs {
    taxonomy_name: Ident,
}

impl MacroArgs {
    pub fn parse(attr_args: TokenStream) -> SynResult<Self> {
        if attr_args.is_empty() {
            Err(syn::Error::new(
                attr_args.span(),
                "taxonomy attribute requires a taxonomy name argument",
            ))
        } else {
            parse2(attr_args)
        }
    }
}

impl Parse for MacroArgs {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        let taxonomy_name: Ident = input.parse()?;

        Ok(Self { taxonomy_name })
    }
}

#[expect(
    missing_docs,
    clippy::missing_errors_doc,
    reason = "this is documented in the data_privacy reexport"
)]
pub fn taxonomy(attr_args: TokenStream, item: TokenStream) -> SynResult<TokenStream> {
    let macro_args = MacroArgs::parse(attr_args)?;
    let input: ItemEnum = parse2(item)?;

    if !input.generics.params.is_empty() {
        return Err(syn::Error::new_spanned(
            &input.generics,
            "the taxonomy attribute cannot be applied to generic enums",
        ));
    }

    let data_privacy_path = quote!(::data_privacy);
    let enum_name = &input.ident;

    let mut taxonomy_variants = Vec::new();

    for variant in &input.variants {
        match &variant.fields {
            Fields::Unit => {}
            _ => {
                return Err(syn::Error::new_spanned(
                    variant,
                    "the taxonomy attribute only supports unit variants",
                ));
            }
        }

        let variant_name = &variant.ident;
        let variant_name_str = variant_name.to_string();
        let snake_case_variant_name = pascal_to_snake_case(&variant_name_str);
        let variant_docs = variant.attrs.iter().filter(|attr| attr.path().is_ident("doc"));

        taxonomy_variants.push((quote!(#(#variant_docs)*), variant_name.clone(), snake_case_variant_name));
    }

    let taxonomy_name = macro_args.taxonomy_name.to_string();

    let data_class_match_arms: Vec<_> = taxonomy_variants
        .iter()
        .map(|(_, variant_name, snake_case)| {
            quote! {
                #enum_name::#variant_name => #data_privacy_path::DataClass::new(#taxonomy_name, #snake_case)
            }
        })
        .collect();

    let data_class_ref_arms: Vec<_> = taxonomy_variants
        .iter()
        .map(|(_, variant_name, snake_case)| {
            quote! {
                #enum_name::#variant_name => const { &#data_privacy_path::DataClass::new(#taxonomy_name, #snake_case) }
            }
        })
        .collect();

    let classification_fns: Vec<_> = taxonomy_variants
        .iter()
        .map(|(docs, variant_name, snake_case)| {
            let fn_name = Ident::new(&format!("classify_{snake_case}"), variant_name.span());
            quote! {
                #docs
                impl #enum_name {
                    #[doc = "Constructs a classified value for this data class."]
                    #[must_use]
                    pub fn #fn_name<T>(payload: T) -> #data_privacy_path::Sensitive<T> {
                        #data_privacy_path::Sensitive::new(payload, #data_privacy_path::DataClass::new(#taxonomy_name, #snake_case))
                    }
                }
            }
        })
        .collect();

    Ok(quote! {
        #input

        impl #enum_name {
            /// Returns the data class associated with the current variant.
            #[must_use]
            pub const fn data_class(&self) -> #data_privacy_path::DataClass {
                match self {
                    #( #data_class_match_arms ),*
                }
            }
        }

        impl ::core::convert::AsRef<#data_privacy_path::DataClass> for #enum_name {
            fn as_ref(&self) -> &#data_privacy_path::DataClass {
                match self {
                    #( #data_class_ref_arms ),*
                }
            }
        }

        impl #data_privacy_path::IntoDataClass for #enum_name {
            fn into_data_class(self) -> #data_privacy_path::DataClass {
                self.data_class()
            }
        }

        impl ::core::cmp::PartialEq<#data_privacy_path::DataClass> for #enum_name {
            fn eq(&self, other: &#data_privacy_path::DataClass) -> core::primitive::bool {
                self.data_class() == *other
            }
        }

        impl ::core::cmp::PartialEq<#enum_name> for #data_privacy_path::DataClass {
            fn eq(&self, other: &#enum_name) -> core::primitive::bool {
                other == self
            }
        }

        #( #classification_fns )*
    })
}

/// Convert `PascalCase` to `snake_case`
fn pascal_to_snake_case(s: &str) -> String {
    let mut result = String::new();
    let chars: Vec<char> = s.chars().collect();

    for (i, ch) in chars.iter().enumerate() {
        if ch.is_uppercase() {
            if i > 0 {
                result.push('_');
            }
            result.extend(ch.to_lowercase());
        } else {
            result.push(*ch);
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use crate::taxonomy::pascal_to_snake_case;

    #[test]
    fn test_pascal_to_snake_case() {
        assert_eq!(pascal_to_snake_case("PascalCase"), "pascal_case");
        assert_eq!(pascal_to_snake_case("AnotherExample"), "another_example");
        assert_eq!(pascal_to_snake_case("Simple"), "simple");
        assert_eq!(pascal_to_snake_case("WithNumbers123"), "with_numbers123");
    }
}
