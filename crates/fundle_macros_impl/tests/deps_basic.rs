// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use syn::{ItemStruct, parse_quote};

mod util;

#[test]
fn basic_expansion() {
    let item: ItemStruct = parse_quote! {
        #[deps]
        struct Foo {
            x: String
        }
    };

    insta::assert_snapshot!(expand_fundle_deps!(item));
}
