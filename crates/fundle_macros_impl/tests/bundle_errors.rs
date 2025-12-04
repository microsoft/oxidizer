// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![expect(missing_docs, reason = "Test code")]

use syn::{ItemStruct, parse_quote};

mod util;

#[test]
#[cfg_attr(miri, ignore)]
fn forward_empty() {
    let item: ItemStruct = parse_quote! {
        #[bundle]
        struct Foo {
            #[forward()]
            x: Bar
        }
    };

    insta::assert_snapshot!(expand_fundle_bundle!(item));
}

#[test]
#[cfg_attr(miri, ignore)]
fn forward_invalid_syntax() {
    let item: ItemStruct = parse_quote! {
        #[bundle]
        struct Foo {
            #[forward(123)]
            x: Bar
        }
    };

    insta::assert_snapshot!(expand_fundle_bundle!(item));
}
