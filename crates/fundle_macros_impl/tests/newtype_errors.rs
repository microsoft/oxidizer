// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![expect(missing_docs, reason = "Test code")]

use syn::{ItemStruct, ItemUnion, parse_quote};

mod util;

#[test]
#[cfg_attr(miri, ignore)]
fn empty_structs() {
    let item: ItemStruct = parse_quote! {
        #[newtype]
        struct Foo;
    };

    insta::assert_snapshot!(expand_fundle_newtype!(item));
}

#[test]
#[cfg_attr(miri, ignore)]
fn multi_field() {
    let item: ItemStruct = parse_quote! {
        #[newtype]
        struct Foo(String, String);
    };

    insta::assert_snapshot!(expand_fundle_newtype!(item));
}

#[test]
#[cfg_attr(miri, ignore)]
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
#[cfg_attr(miri, ignore)]
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
