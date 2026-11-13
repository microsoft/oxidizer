// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Tests behavior through the public API.

use bytes::BufMut;
use bytesbuf::mem::testing::TransparentMemory;

#[test]
fn buf_mut_compat() {
    let memory = TransparentMemory::new();
    let mut buf = memory.reserve(100);

    assert_eq!(BufMut::remaining_mut(&buf), 100);

    // 100 + 100
    buf.reserve(200, &memory);

    assert_eq!(BufMut::remaining_mut(&buf), 200);

    let chunk = BufMut::chunk_mut(&mut buf);
    assert_eq!(chunk.len(), 100);

    // SAFETY: Lies - we did not write anything. But we will also
    // not touch the data - we are only inspecting the bookkeeping.
    // Good enough for test code.
    unsafe {
        BufMut::advance_mut(&mut buf, 50);
    }

    let chunk = BufMut::chunk_mut(&mut buf);
    assert_eq!(chunk.len(), 50);

    // SAFETY: See above.
    unsafe {
        BufMut::advance_mut(&mut buf, 50);
    }

    let chunk = BufMut::chunk_mut(&mut buf);
    assert_eq!(chunk.len(), 100);

    // SAFETY: See above.
    unsafe {
        BufMut::advance_mut(&mut buf, 100);
    }

    let chunk = BufMut::chunk_mut(&mut buf);
    assert_eq!(chunk.len(), 0);
}
