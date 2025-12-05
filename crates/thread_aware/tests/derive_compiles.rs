// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![expect(missing_docs, reason = "Test code")]

use thread_aware::{ThreadAware, create_manual_pinned_affinities};
use thread_aware_macros::ThreadAware as TA;

#[derive(TA)]
struct Inner(u32);

#[derive(TA)]
struct Container<T: ThreadAware> {
    val: T,
    #[thread_aware(skip)]
    raw: usize,
}

#[test]
fn derive_thread_aware_compiles_and_calls() {
    let mut addrs = create_manual_pinned_affinities(&[2]);
    let a = addrs.remove(0).into();
    let b = addrs.remove(0);
    let c = Container { val: Inner(5), raw: 10 };
    let _moved = c.relocated(a, b);
}
