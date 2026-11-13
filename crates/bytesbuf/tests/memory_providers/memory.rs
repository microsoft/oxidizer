// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Tests behavior through the public API.

use std::sync::atomic::{AtomicUsize, Ordering};

use bytesbuf::BytesBuf;
use bytesbuf::mem::Memory;
use bytesbuf::mem::testing::TransparentMemory;

#[derive(Debug)]
struct CountingMemory {
    reserve_calls: AtomicUsize,
    inner: TransparentMemory,
}

impl CountingMemory {
    fn new() -> Self {
        Self {
            reserve_calls: AtomicUsize::new(0),
            inner: TransparentMemory::new(),
        }
    }
}

impl Memory for CountingMemory {
    fn reserve(&self, min_bytes: usize) -> BytesBuf {
        self.reserve_calls.fetch_add(1, Ordering::SeqCst);
        self.inner.reserve(min_bytes + 16)
    }
}

fn reserve_from_generic<M: Memory>(memory: M, min_bytes: usize) -> BytesBuf {
    memory.reserve(min_bytes)
}

#[test]
fn memory_impl_for_reference_forwards_reserve_to_underlying() {
    let memory = CountingMemory::new();
    let min_bytes = 64;

    let buf = reserve_from_generic(&memory, min_bytes);

    assert_eq!(memory.reserve_calls.load(Ordering::SeqCst), 1);
    assert!(buf.capacity() >= min_bytes + 16);
}
