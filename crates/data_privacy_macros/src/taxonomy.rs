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

#[allow(clippy::too_many_lines, reason = "Yeah, it's a bit much...")]
pub fn taxonomy_impl(attr_args: TokenStream, item: TokenStream) -> SynResult<TokenStream> {
    let macro_args = MacroArgs::parse(attr_args)?;
    let input: ItemEnum = parse2(item)?;

    if !input.generics.params.is_empty() {
        return Err(syn::Error::new_spanned(
            &input.generics,
            "the taxonomy attribute cannot be applied to generic enums",
        ));
    }

    let data_privacy_path = quote!(data_privacy);
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

    let classification_fns: Vec<_> = taxonomy_variants
        .iter()
        .map(|(docs, variant_name, snake_case)| {
            let fn_name = Ident::new(&format!("classify_{snake_case}"), variant_name.span());
            quote! {
                #docs
                impl #enum_name {
                    #[doc = "Constructs a classified value for this data class."]
                    #[must_use]
                    pub fn #fn_name<T>(payload: T) -> #data_privacy_path::ClassifiedWrapper<T> {
                        #data_privacy_path::ClassifiedWrapper::new(payload, #data_privacy_path::DataClass::new(#taxonomy_name, #snake_case))
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

        impl core::cmp::PartialEq<#data_privacy_path::DataClass> for #enum_name {
            fn eq(&self, other: &#data_privacy_path::DataClass) -> core::primitive::bool {
                self.data_class() == *other
            }
        }

        impl core::cmp::PartialEq<#enum_name> for #data_privacy_path::DataClass {
            fn eq(&self, other: &#enum_name) -> core::primitive::bool {
                other == self
            }
        }

        #( #classification_fns )*
    })
}

#[cfg(test)]
mod tests {
    use insta::assert_snapshot;

    use super::*;

    #[test]
    fn test_pascal_to_snake_case() {
        assert_eq!(pascal_to_snake_case("PascalCase"), "pascal_case");
        assert_eq!(pascal_to_snake_case("AnotherExample"), "another_example");
        assert_eq!(pascal_to_snake_case("Simple"), "simple");
        assert_eq!(pascal_to_snake_case("WithNumbers123"), "with_numbers123");
    }

    #[test]
    fn test_taxonomy_impl_empty_args() {
        let input = quote! {
            pub enum MyEnum {
                VariantOne,
                VariantTwo,
            }
        };

        let attr_args = quote! {};
        let result = taxonomy_impl(attr_args, input);

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("taxonomy attribute requires a taxonomy name argument"));
    }

    #[test]
    fn test_taxonomy_impl_invalid_taxonomy_name() {
        let input = quote! {
            pub enum MyEnum {
                VariantOne,
                VariantTwo,
            }
        };

        let attr_args = quote! { "InvalidName" };
        let result = taxonomy_impl(attr_args, input);

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!("expected identifier", err.to_string());
    }

    #[test]
    fn test_taxonomy_impl_missing_comma() {
        let input = quote! {
            pub enum MyEnum {
                VariantOne,
                VariantTwo,
            }
        };

        let attr_args = quote! { MyTaxonomy serde = true };
        let result = taxonomy_impl(attr_args, input);

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!("unexpected token", err.to_string());
    }

    #[test]
    fn test_taxonomy_impl_non_enum_struct() {
        let input = quote! {
            pub struct MyStruct {
                field: i32,
            }
        };

        let attr_args = quote! { MyTaxonomy };
        let result = taxonomy_impl(attr_args, input);

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("expected `enum`"));
    }

    #[test]
    fn test_taxonomy_impl_generic_enum() {
        let input = quote! {
            pub enum MyEnum<T> {
                VariantOne(T),
                VariantTwo,
            }
        };

        let attr_args = quote! { MyTaxonomy };
        let result = taxonomy_impl(attr_args, input);

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            err.to_string()
                .contains("the taxonomy attribute cannot be applied to generic enums")
        );
    }

    #[test]
    fn test_taxonomy_impl_non_unit_variant_named() {
        let input = quote! {
            pub enum MyEnum {
                VariantOne { field: i32 },
                VariantTwo,
            }
        };

        let attr_args = quote! { MyTaxonomy };
        let result = taxonomy_impl(attr_args, input);

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("the taxonomy attribute only supports unit variants"));
    }

    #[test]
    fn test_taxonomy_impl_non_unit_variant_unnamed() {
        let input = quote! {
            pub enum MyEnum {
                VariantOne(i32),
                VariantTwo,
            }
        };

        let attr_args = quote! { MyTaxonomy };
        let result = taxonomy_impl(attr_args, input);

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("the taxonomy attribute only supports unit variants"));
    }

    #[test]
    fn test_taxonomy_impl_invalid_syn_parse() {
        let input = quote! {
            invalid rust syntax here
        };

        let attr_args = quote! { MyTaxonomy };
        let result = taxonomy_impl(attr_args, input);

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!("expected `enum`", err.to_string());
    }

    #[test]
    fn test_success() {
        let args = quote! { tax };
        let input = quote! {
            enum GovTaxonomy {
                #[doc("Really secret data")]
                Confidential,
                #[doc("More secret data")]
                TopSecret,
            }
        };

        let result = taxonomy_impl(args, input);
        let result_file = syn::parse_file(&result.unwrap().to_string()).unwrap();
        let pretty = prettyplease::unparse(&result_file);

        assert_snapshot!(pretty);
    }
}
