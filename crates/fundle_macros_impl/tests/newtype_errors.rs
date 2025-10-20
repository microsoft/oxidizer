// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use syn::{parse_quote, ItemStruct, ItemUnion};

mod util;

#[test]
fn empty_structs() {
    let item: ItemStruct = parse_quote! {
        #[newtype]
        struct Foo;
    };

    insta::assert_snapshot!(expand_fundle_newtype!(item));
}

#[test]
fn multi_field() {
    let item: ItemStruct = parse_quote! {
        #[newtype]
        struct Foo(String, String);
    };

    insta::assert_snapshot!(expand_fundle_newtype!(item));
}

#[test]
fn multi_field_named() {
    let item: ItemStruct = parse_quote! {
        #[newtype]
        struct Foo {
            x: String,
            y: String
        }
    };

    insta::assert_snapshot!(expand_fundle_newtype!(item));
}


#[test]
fn union() {
    let item: ItemUnion = parse_quote! {
        #[newtype]
        union Foo {
            x: String,
            y: String
        }
    };

    insta::assert_snapshot!(expand_fundle_newtype!(item));
}