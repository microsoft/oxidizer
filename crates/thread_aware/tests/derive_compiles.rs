// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![expect(missing_docs, reason = "This is a test module")]
#![allow(dead_code, reason = "This is a test module")]

use thread_aware::affinity::create_manual_pinned_affinities;
use thread_aware::ThreadAware;

#[derive(ThreadAware)]
struct Inner(u32);

#[derive(ThreadAware)]
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
