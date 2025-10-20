// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use syn::{parse_quote, ItemStruct, ItemUnion};

mod util;

#[test]
fn empty() {
    let item: ItemStruct = parse_quote! {
        #[deps]
        struct Foo {}
    };

    insta::assert_snapshot!(expand_fundle_deps!(item));
}

#[test]
fn tuple() {
    let item: ItemStruct = parse_quote! {
        #[deps]
        struct Foo(u8);
    };

    insta::assert_snapshot!(expand_fundle_deps!(item));
}

#[test]
fn unit() {
    let item: ItemStruct = parse_quote! {
        #[deps]
        struct Foo;
    };

    insta::assert_snapshot!(expand_fundle_deps!(item));
}


#[test]
fn union() {
    let item: ItemUnion = parse_quote! {
        #[deps]
        union Foo {}
    };

    insta::assert_snapshot!(expand_fundle_deps!(item));
}