// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![expect(missing_docs, reason = "Test code")]

use syn::{ItemStruct, parse_quote};

mod util;

#[test]
#[cfg_attr(miri, ignore)]
fn basic_expansion() {
    let item: ItemStruct = parse_quote! {
        #[bundle]
        struct Foo {}
    };

    insta::assert_snapshot!(expand_fundle_bundle!(item));
}

#[test]
#[cfg_attr(miri, ignore)]
fn pub_expansion() {
    let item: ItemStruct = parse_quote! {
        #[bundle]
        pub struct Foo {}
    };

    insta::assert_snapshot!(expand_fundle_bundle!(item));
}
