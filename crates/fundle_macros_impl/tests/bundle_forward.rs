// Copyright (c) Microsoft Corporation.

use syn::{parse_quote,  ItemStruct};

mod util;

#[test]
fn foward_explicit() {
    let item: ItemStruct = parse_quote! {
        #[bundle]
        struct Foo {
            #[forward(u8)]
            x: Bar
        }
    };

    insta::assert_snapshot!(expand_fundle_bundle!(item));
}