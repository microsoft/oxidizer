// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use data_privacy_macros_impl::taxonomy::taxonomy;
use insta::assert_snapshot;
use quote::quote;

#[test]
fn test_taxonomy_impl_empty_args() {
    let input = quote! {
        pub enum MyEnum {
            VariantOne,
            VariantTwo,
        }
    };

    let attr_args = quote! {};
    let result = taxonomy(attr_args, input);

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
    let result = taxonomy(attr_args, input);

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
    let result = taxonomy(attr_args, input);

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
    let result = taxonomy(attr_args, input);

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
    let result = taxonomy(attr_args, input);

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
    let result = taxonomy(attr_args, input);

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
    let result = taxonomy(attr_args, input);

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
    let result = taxonomy(attr_args, input);

    assert!(result.is_err());
    let err = result.unwrap_err();
    assert_eq!("expected `enum`", err.to_string());
}

#[test]
#[cfg_attr(miri, ignore)]
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

    let result = taxonomy(args, input);
    let result_file = syn::parse_file(&result.unwrap().to_string()).unwrap();
    let pretty = prettyplease::unparse(&result_file);

    assert_snapshot!(pretty);
}
