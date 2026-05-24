// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![expect(missing_docs, reason = "Test code")]

use syn::{ImplItemFn, parse_quote};

mod util;

#[test]
#[cfg_attr(miri, ignore)]
fn from_field_arc_self() {
    let item: ImplItemFn = parse_quote! {
        #[thunk(from = me.thunker)]
        async fn work(me: ::std::sync::Arc<Service>) -> u64 {
            42
        }
    };

    insta::assert_snapshot!(expand_thunk!(item));
}

#[test]
#[cfg_attr(miri, ignore)]
fn ref_self_is_rejected() {
    let item: ImplItemFn = parse_quote! {
        #[thunk(from = me.thunker)]
        async fn work(&self) -> String {
            String::from("hello")
        }
    };

    insta::assert_snapshot!(expand_thunk!(item));
}

#[test]
#[cfg_attr(miri, ignore)]
fn mut_self_is_rejected() {
    let item: ImplItemFn = parse_quote! {
        #[thunk(from = me.thunker)]
        async fn work(&mut self) -> String {
            String::from("hello")
        }
    };

    insta::assert_snapshot!(expand_thunk!(item));
}

#[test]
#[cfg_attr(miri, ignore)]
fn owned_self_is_rejected() {
    let item: ImplItemFn = parse_quote! {
        #[thunk(from = me.thunker)]
        async fn work(self) -> u64 {
            42
        }
    };

    insta::assert_snapshot!(expand_thunk!(item));
}

#[test]
#[cfg_attr(miri, ignore)]
fn typed_self_is_rejected() {
    let item: ImplItemFn = parse_quote! {
        #[thunk(from = me.thunker)]
        async fn work(self: ::std::sync::Arc<Self>) -> u64 {
            42
        }
    };

    insta::assert_snapshot!(expand_thunk!(item));
}

#[test]
#[cfg_attr(miri, ignore)]
fn from_method_call() {
    let item: ImplItemFn = parse_quote! {
        #[thunk(from = me.thunker())]
        async fn work(me: ::std::sync::Arc<Service>) -> u32 {
            1
        }
    };

    insta::assert_snapshot!(expand_thunk!(item));
}

#[test]
#[cfg_attr(miri, ignore)]
fn from_parameter() {
    let item: ImplItemFn = parse_quote! {
        #[thunk(from = thunker)]
        async fn create(thunker: ::std::sync::Arc<Thunker>, name: String) -> Self {
            Self { name }
        }
    };

    insta::assert_snapshot!(expand_thunk!(item));
}

#[test]
#[cfg_attr(miri, ignore)]
fn from_static() {
    let item: ImplItemFn = parse_quote! {
        #[thunk(from = THUNKER)]
        async fn work() -> bool {
            true
        }
    };

    insta::assert_snapshot!(expand_thunk!(item));
}

#[test]
#[cfg_attr(miri, ignore)]
fn shared_ref_param_is_rejected() {
    let item: ImplItemFn = parse_quote! {
        #[thunk(from = me.thunker)]
        async fn read(me: ::std::sync::Arc<Service>, path: &Path) -> Vec<u8> {
            vec![]
        }
    };

    insta::assert_snapshot!(expand_thunk!(item));
}

#[test]
#[cfg_attr(miri, ignore)]
fn mut_ref_param_is_rejected() {
    let item: ImplItemFn = parse_quote! {
        #[thunk(from = me.thunker)]
        async fn fill(me: ::std::sync::Arc<Service>, buf: &mut Vec<u8>) -> usize {
            buf.push(1);
            1
        }
    };

    insta::assert_snapshot!(expand_thunk!(item));
}

#[test]
#[cfg_attr(miri, ignore)]
fn owned_param() {
    let item: ImplItemFn = parse_quote! {
        #[thunk(from = me.thunker)]
        async fn consume(me: ::std::sync::Arc<Service>, data: Vec<u8>) -> usize {
            data.len()
        }
    };

    insta::assert_snapshot!(expand_thunk!(item));
}

#[test]
#[cfg_attr(miri, ignore)]
fn multiple_owned_params() {
    let item: ImplItemFn = parse_quote! {
        #[thunk(from = me.thunker)]
        async fn multi(me: ::std::sync::Arc<Service>, a: u32, b: String, c: Vec<u8>) -> bool {
            true
        }
    };

    insta::assert_snapshot!(expand_thunk!(item));
}

#[test]
#[cfg_attr(miri, ignore)]
fn unit_return_type() {
    let item: ImplItemFn = parse_quote! {
        #[thunk(from = me.thunker)]
        async fn fire_and_forget(me: ::std::sync::Arc<Service>) {
            // no return
        }
    };

    insta::assert_snapshot!(expand_thunk!(item));
}

#[test]
#[cfg_attr(miri, ignore)]
fn result_return_type() {
    let item: ImplItemFn = parse_quote! {
        #[thunk(from = me.thunker)]
        async fn fallible(me: ::std::sync::Arc<Service>) -> std::io::Result<Vec<u8>> {
            Ok(vec![])
        }
    };

    insta::assert_snapshot!(expand_thunk!(item));
}

#[test]
#[cfg_attr(miri, ignore)]
fn pub_visibility() {
    let item: ImplItemFn = parse_quote! {
        #[thunk(from = me.thunker)]
        pub async fn public_work(me: ::std::sync::Arc<Service>) -> u64 {
            42
        }
    };

    insta::assert_snapshot!(expand_thunk!(item));
}

#[test]
#[cfg_attr(miri, ignore)]
fn pub_crate_visibility() {
    let item: ImplItemFn = parse_quote! {
        #[thunk(from = me.thunker)]
        pub(crate) async fn internal_work(me: ::std::sync::Arc<Service>) -> u64 {
            42
        }
    };

    insta::assert_snapshot!(expand_thunk!(item));
}

#[test]
#[cfg_attr(miri, ignore)]
fn no_extra_params() {
    let item: ImplItemFn = parse_quote! {
        #[thunk(from = me.thunker)]
        async fn simple(me: ::std::sync::Arc<Service>) -> i32 {
            -1
        }
    };

    insta::assert_snapshot!(expand_thunk!(item));
}

#[test]
#[cfg_attr(miri, ignore)]
fn from_parameter_multiple_args() {
    let item: ImplItemFn = parse_quote! {
        #[thunk(from = thunker)]
        async fn create(thunker: ::std::sync::Arc<Thunker>, path: ::std::path::PathBuf, mode: u32) -> std::io::Result<Self> {
            Ok(Self)
        }
    };

    insta::assert_snapshot!(expand_thunk!(item));
}

#[test]
#[cfg_attr(miri, ignore)]
fn preserves_allow_attrs() {
    let item: ImplItemFn = parse_quote! {
        #[allow(unused)]
        #[thunk(from = me.thunker)]
        async fn with_allow(me: ::std::sync::Arc<Service>) -> u64 {
            42
        }
    };

    insta::assert_snapshot!(expand_thunk!(item));
}

#[test]
#[cfg_attr(miri, ignore)]
fn preserves_expect_attrs() {
    let item: ImplItemFn = parse_quote! {
        #[expect(clippy::needless_pass_by_value, reason = "testing")]
        #[thunk(from = me.thunker)]
        async fn with_expect(me: ::std::sync::Arc<Service>, data: Vec<u8>) -> usize {
            data.len()
        }
    };

    insta::assert_snapshot!(expand_thunk!(item));
}

#[test]
#[cfg_attr(miri, ignore)]
fn generic_return_type() {
    let item: ImplItemFn = parse_quote! {
        #[thunk(from = me.thunker)]
        async fn get_option(me: ::std::sync::Arc<Service>) -> Option<String> {
            Some(String::from("hi"))
        }
    };

    insta::assert_snapshot!(expand_thunk!(item));
}
