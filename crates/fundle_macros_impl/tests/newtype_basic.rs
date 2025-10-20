// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use syn::{ItemStruct, parse_quote};

mod util;

#[test]
#[cfg_attr(miri, ignore)]
fn basic_expansion() {
    let item: ItemStruct = parse_quote! {
        #[newtype]
        struct Foo(String);
    };

    insta::assert_snapshot!(expand_fundle_newtype!(item));
}
