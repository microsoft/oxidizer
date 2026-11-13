// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Tests behavior through the public API.

use bytesbuf::BytesView;
use bytesbuf::mem::MemoryShared;
use bytesbuf::mem::testing::FixedBlockMemory;
use new_zealand::nz;
use static_assertions::assert_impl_all;
use thread_aware::ThreadAware;

assert_impl_all!(FixedBlockMemory: MemoryShared, ThreadAware);

#[test]
fn byte_by_byte() {
    let memory = FixedBlockMemory::new(nz!(1));

    let buf = memory.reserve(0);
    assert_eq!(buf.len(), 0);
    assert_eq!(buf.capacity(), 0);

    let mut data = BytesView::copied_from_slice(b"Hello, world", &memory);
    assert_eq!(data, b"Hello, world");

    assert_eq!(data.first_slice().len(), 1);

    let mut chunks_encountered: usize = 0;

    data.consume_all_slices(|chunk| {
        chunks_encountered = chunks_encountered.saturating_add(1);
        assert_eq!(chunk.len(), 1);
    });

    assert_eq!(chunks_encountered, 12);
}
