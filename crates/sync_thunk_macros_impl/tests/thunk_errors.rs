// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![expect(missing_docs, reason = "Test code")]

mod util;

#[test]
#[cfg_attr(miri, ignore)]
fn missing_from() {
    let attr_args = quote::quote! {};
    let item = quote::quote! {
        async fn work(&self) -> u64 {
            42
        }
    };

    insta::assert_snapshot!(expand_thunk_error!(attr_args, item));
}

#[test]
#[cfg_attr(miri, ignore)]
fn unknown_key() {
    let attr_args = quote::quote! { foo = bar };
    let item = quote::quote! {
        async fn work(&self) -> u64 {
            42
        }
    };

    insta::assert_snapshot!(expand_thunk_error!(attr_args, item));
}

#[test]
#[cfg_attr(miri, ignore)]
fn missing_equals() {
    let attr_args = quote::quote! { from };
    let item = quote::quote! {
        async fn work(&self) -> u64 {
            42
        }
    };

    insta::assert_snapshot!(expand_thunk_error!(attr_args, item));
}

#[test]
#[cfg_attr(miri, ignore)]
fn not_a_function() {
    let attr_args = quote::quote! { from = self.thunker };
    let item = quote::quote! {
        struct NotAFunction;
    };

    insta::assert_snapshot!(expand_thunk_error!(attr_args, item));
}
