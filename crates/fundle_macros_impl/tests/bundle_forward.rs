// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![expect(missing_docs, reason = "Test code")]

use syn::{ItemStruct, parse_quote};

mod util;

#[test]
#[cfg_attr(miri, ignore)]
fn forward_explicit() {
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
#[cfg_attr(miri, ignore)]
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
