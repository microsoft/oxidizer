// Copyright (c) Microsoft Corporation.

use syn::{parse_quote,  ItemStruct};

mod util;

#[test]
fn foward_explicit() {
    let item: ItemStruct = parse_quote! {
        #[bundle]
        struct Foo {
            #[forward(u8, u16)]
            x: Bar,
            #[something_else]
            y: Bar
        }
    };

    insta::assert_snapshot!(expand_fundle_bundle!(item));
}

#[test]
fn unrelated_attr() {
    let item: ItemStruct = parse_quote! {
        #[bundle]
        struct Foo {
            #[something_else]
            x: Bar
        }
    };

    insta::assert_snapshot!(expand_fundle_bundle!(item));
}