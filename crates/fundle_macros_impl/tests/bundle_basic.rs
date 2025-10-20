// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use syn::{ItemStruct, parse_quote};

mod util;

#[test]
fn basic_expansion() {
    let item: ItemStruct = parse_quote! {
        #[bundle]
        struct Foo {}
    };

    insta::assert_snapshot!(expand_fundle_bundle!(item));
}
