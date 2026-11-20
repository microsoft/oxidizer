// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![expect(missing_docs, reason = "Test code")]

use data_privacy_macros_impl::taxonomy::taxonomy;
use insta::assert_snapshot;
use quote::quote;
use testing_aids::render_expansion;

#[test]
fn test_pascal_to_snake_case() {
    let expansion = taxonomy(
        quote!(test),
        quote! {
            enum TestTaxonomy {
                PascalCase,
                AnotherExample,
                Simple,
                WithNumbers123,
            }
        },
    )
    .unwrap();
    let generated: syn::File = syn::parse2(expansion).unwrap();
    let names: Vec<_> = generated
        .items
        .iter()
        .filter_map(|item| match item {
            syn::Item::Impl(item) if item.trait_.is_none() => Some(item),
            _ => None,
        })
        .flat_map(|item| &item.items)
        .filter_map(|item| match item {
            syn::ImplItem::Fn(function) if function.sig.ident == "data_class" => Some(function),
            _ => None,
        })
        .flat_map(|function| &function.block.stmts)
        .filter_map(|statement| match statement {
            syn::Stmt::Expr(syn::Expr::Match(expression), _) => Some(expression),
            _ => None,
        })
        .flat_map(|expression| &expression.arms)
        .map(|arm| {
            let syn::Expr::Call(call) = &*arm.body else {
                panic!("each data class must be constructed");
            };
            let Some(syn::Expr::Lit(syn::ExprLit {
                lit: syn::Lit::Str(name), ..
            })) = call.args.last()
            else {
                panic!("the data class name must be a string");
            };
            name.value()
        })
        .collect();

    assert_eq!(names.len(), 4);
    assert_eq!(names[0], "pascal_case");
    assert_eq!(names[1], "another_example");
    assert_eq!(names[2], "simple");
    assert_eq!(names[3], "with_numbers123");
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
    let pretty = render_expansion(&result.unwrap());

    assert_snapshot!(pretty);
}
