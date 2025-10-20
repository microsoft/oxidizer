// Copyright (c) Microsoft Corporation.

use syn::{parse_quote,  ItemStruct};

mod util;

#[test]
fn basic_expansion() {
    let item: ItemStruct = parse_quote! {
        #[fundle::bundle]
        struct Foo {}
    };

    insta::assert_snapshot!(expand_fundle_bundle!(item));
}