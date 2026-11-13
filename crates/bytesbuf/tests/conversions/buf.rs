// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Tests behavior through the public API.

use bytes::BufMut;

use crate::test_memory::TestMemory;

#[test]
fn buf_mut_compat() {
    let memory = TestMemory::new();
    let mut buf = memory.reserve(100);

    assert_eq!(BufMut::remaining_mut(&buf), 100);

    // 100 + 100
    buf.reserve(200, &memory);

    assert_eq!(BufMut::remaining_mut(&buf), 200);

    let chunk = BufMut::chunk_mut(&mut buf);
    assert_eq!(chunk.len(), 100);
    chunk.copy_from_slice(&[0; 100]);

    // SAFETY: The first 50 bytes of this chunk were initialized above.
    unsafe {
        BufMut::advance_mut(&mut buf, 50);
    }

    let chunk = BufMut::chunk_mut(&mut buf);
    assert_eq!(chunk.len(), 50);
    chunk.copy_from_slice(&[0; 50]);

    // SAFETY: All 50 bytes of this chunk were initialized above.
    unsafe {
        BufMut::advance_mut(&mut buf, 50);
    }

    let chunk = BufMut::chunk_mut(&mut buf);
    assert_eq!(chunk.len(), 100);
    chunk.copy_from_slice(&[0; 100]);

    // SAFETY: All 100 bytes of this chunk were initialized above.
    unsafe {
        BufMut::advance_mut(&mut buf, 100);
    }

    let chunk = BufMut::chunk_mut(&mut buf);
    assert_eq!(chunk.len(), 0);
}
