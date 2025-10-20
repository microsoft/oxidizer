// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use syn::{ItemStruct, ItemUnion, parse_quote};

mod util;

#[test]
#[cfg_attr(miri, ignore)]
fn empty() {
    let item: ItemStruct = parse_quote! {
        #[deps]
        struct Foo {}
    };

    insta::assert_snapshot!(expand_fundle_deps!(item));
}

#[test]
#[cfg_attr(miri, ignore)]
fn tuple() {
    let item: ItemStruct = parse_quote! {
        #[deps]
        struct Foo(u8);
    };

    insta::assert_snapshot!(expand_fundle_deps!(item));
}

#[test]
#[cfg_attr(miri, ignore)]
fn unit() {
    let item: ItemStruct = parse_quote! {
        #[deps]
        struct Foo;
    };

    insta::assert_snapshot!(expand_fundle_deps!(item));
}

#[test]
#[cfg_attr(miri, ignore)]
fn union() {
    let item: ItemUnion = parse_quote! {
        #[deps]
        union Foo {}
    };

    insta::assert_snapshot!(expand_fundle_deps!(item));
}
