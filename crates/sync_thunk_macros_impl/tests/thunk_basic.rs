// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![expect(missing_docs, reason = "Test code")]

use syn::{ImplItemFn, parse_quote};

mod util;

#[test]
#[cfg_attr(miri, ignore)]
fn from_field_ref_self() {
    let item: ImplItemFn = parse_quote! {
        #[thunk(from = self.thunker)]
        async fn work(&self) -> u64 {
            42
        }
    };

    insta::assert_snapshot!(expand_thunk!(item));
}

#[test]
#[cfg_attr(miri, ignore)]
fn from_field_mut_self() {
    let item: ImplItemFn = parse_quote! {
        #[thunk(from = self.thunker)]
        async fn work(&mut self) -> String {
            String::from("hello")
        }
    };

    insta::assert_snapshot!(expand_thunk!(item));
}

#[test]
#[cfg_attr(miri, ignore)]
fn from_method_call() {
    let item: ImplItemFn = parse_quote! {
        #[thunk(from = self.thunker())]
        async fn work(&self) -> u32 {
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
        async fn create(thunker: &Thunker, name: String) -> Self {
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
        async fn work(&self) -> bool {
            true
        }
    };

    insta::assert_snapshot!(expand_thunk!(item));
}

#[test]
#[cfg_attr(miri, ignore)]
fn ref_param() {
    let item: ImplItemFn = parse_quote! {
        #[thunk(from = self.thunker)]
        async fn read(&self, path: &Path) -> Vec<u8> {
            vec![]
        }
    };

    insta::assert_snapshot!(expand_thunk!(item));
}

#[test]
#[cfg_attr(miri, ignore)]
fn mut_ref_param() {
    let item: ImplItemFn = parse_quote! {
        #[thunk(from = self.thunker)]
        async fn fill(&self, buf: &mut Vec<u8>) -> usize {
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
        #[thunk(from = self.thunker)]
        async fn consume(&self, data: Vec<u8>) -> usize {
            data.len()
        }
    };

    insta::assert_snapshot!(expand_thunk!(item));
}

#[test]
#[cfg_attr(miri, ignore)]
fn multiple_params() {
    let item: ImplItemFn = parse_quote! {
        #[thunk(from = self.thunker)]
        async fn multi(&self, a: u32, b: &str, c: &mut Vec<u8>) -> bool {
            true
        }
    };

    insta::assert_snapshot!(expand_thunk!(item));
}

#[test]
#[cfg_attr(miri, ignore)]
fn unit_return_type() {
    let item: ImplItemFn = parse_quote! {
        #[thunk(from = self.thunker)]
        async fn fire_and_forget(&self) {
            // no return
        }
    };

    insta::assert_snapshot!(expand_thunk!(item));
}

#[test]
#[cfg_attr(miri, ignore)]
fn result_return_type() {
    let item: ImplItemFn = parse_quote! {
        #[thunk(from = self.thunker)]
        async fn fallible(&self) -> std::io::Result<Vec<u8>> {
            Ok(vec![])
        }
    };

    insta::assert_snapshot!(expand_thunk!(item));
}

#[test]
#[cfg_attr(miri, ignore)]
fn pub_visibility() {
    let item: ImplItemFn = parse_quote! {
        #[thunk(from = self.thunker)]
        pub async fn public_work(&self) -> u64 {
            42
        }
    };

    insta::assert_snapshot!(expand_thunk!(item));
}

#[test]
#[cfg_attr(miri, ignore)]
fn pub_crate_visibility() {
    let item: ImplItemFn = parse_quote! {
        #[thunk(from = self.thunker)]
        pub(crate) async fn internal_work(&self) -> u64 {
            42
        }
    };

    insta::assert_snapshot!(expand_thunk!(item));
}

#[test]
#[cfg_attr(miri, ignore)]
fn no_params_beyond_self() {
    let item: ImplItemFn = parse_quote! {
        #[thunk(from = self.thunker)]
        async fn simple(&self) -> i32 {
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
        async fn create(thunker: &Thunker, path: &Path, mode: u32) -> std::io::Result<Self> {
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
        #[thunk(from = self.thunker)]
        async fn with_allow(&self) -> u64 {
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
        #[thunk(from = self.thunker)]
        async fn with_expect(&self, data: Vec<u8>) -> usize {
            data.len()
        }
    };

    insta::assert_snapshot!(expand_thunk!(item));
}

#[test]
#[cfg_attr(miri, ignore)]
fn generic_return_type() {
    let item: ImplItemFn = parse_quote! {
        #[thunk(from = self.thunker)]
        async fn get_option(&self) -> Option<String> {
            Some(String::from("hi"))
        }
    };

    insta::assert_snapshot!(expand_thunk!(item));
}
