// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Public test builders accept a pool without an explicit adapter.

#![cfg(feature = "test-util")]

use bytesbuf::BytesView;
use bytesbuf_io::testing::{FakeRead, FakeWrite, Null, Pending};

fn memory() -> &'static bytesbuf::mem::GlobalPool {
    static MEMORY: std::sync::OnceLock<bytesbuf::mem::GlobalPool> = std::sync::OnceLock::new();

    MEMORY.get_or_init(bytesbuf::mem::GlobalPool::new)
}

#[test]
fn stream_builders_accept_global_pools() {
    let pool = memory().clone();
    let reader = FakeRead::builder().contents(BytesView::new()).memory(pool.clone()).build();
    let writer = FakeWrite::builder().memory(pool.clone()).build();
    let null = Null::builder().memory(pool.clone()).build();
    let pending = Pending::builder().memory(pool).build();

    assert!(reader.reserve(17).capacity() >= 17);
    assert!(writer.reserve(17).capacity() >= 17);
    assert!(null.reserve(17).capacity() >= 17);
    assert!(pending.reserve(17).capacity() >= 17);
}
