// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![expect(missing_docs, reason = "Test code")]

use syn::{ItemStruct, parse_quote};

mod util;

#[test]
#[cfg_attr(miri, ignore)]
fn lifetimes() {
    let item: ItemStruct = parse_quote! {
        #[deps]
        struct Foo<'a> {
            x: &'a u32
        }
    };

    insta::assert_snapshot!(expand_fundle_deps!(item));
}

#[test]
#[cfg_attr(miri, ignore)]
fn generics() {
    let item: ItemStruct = parse_quote! {
        #[deps]
        struct Foo<T> {
            x: T
        }
    };

    insta::assert_snapshot!(expand_fundle_deps!(item));
}

#[test]
#[cfg_attr(miri, ignore)]
fn generics_where() {
    let item: ItemStruct = parse_quote! {
        #[deps]
        struct Foo<T> where T: Clone {
            x: T
        }
    };

    insta::assert_snapshot!(expand_fundle_deps!(item));
}
