// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Public memory provider conversions.

#![cfg(feature = "std")]

use bytesbuf::mem::{GlobalPool, OpaqueMemory};

#[test]
fn converted_global_pool_and_its_clone_reserve_writable_memory() {
    let memory = OpaqueMemory::from(GlobalPool::new());
    let cloned = memory.clone();
    drop(memory);

    for size in [0, 1, 1024, 65_537] {
        let mut buffer = cloned.reserve(size);
        assert!(buffer.capacity() >= size);
        assert!(buffer.is_empty());
        let payload = vec![42; size];
        buffer.put_slice(payload.as_slice());
        assert_eq!(buffer.consume_all(), payload.as_slice());
    }
}
