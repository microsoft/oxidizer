// Copyright (c) Microsoft Corporation.

use syn::{parse_quote,  ItemStruct};

mod util;

#[test]
fn generic_lifetimes() {
    let item: ItemStruct = parse_quote! {
        #[deps]
        struct Foo<'a> {
            x: &'a u32
        }
    };

    insta::assert_snapshot!(expand_fundle_deps!(item));
}