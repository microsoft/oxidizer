// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Tests behavior through the public API.

use bytesbuf::mem::{BlockSize, GlobalPool, MemoryShared};
use static_assertions::assert_impl_all;
use thread_aware::{Relocator, ThreadAware};

assert_impl_all!(GlobalPool: MemoryShared, ThreadAware);

#[test]
fn smoke_test() {
    let memory = GlobalPool::new();

    // Nothing to assert with 0-sized, any capacity is acceptable. Just as long as it works.
    _ = memory.reserve(0);

    let builder = memory.reserve(1);
    assert!(builder.capacity() >= 1);

    let builder = memory.reserve(100);
    assert!(builder.capacity() >= 100);

    let builder = memory.reserve(1000);
    assert!(builder.capacity() >= 1000);

    let builder = memory.reserve(10000);
    assert!(builder.capacity() >= 10000);

    let builder = memory.reserve(100_000);
    assert!(builder.capacity() >= 100_000);

    let builder = memory.reserve(1_000_000);
    assert!(builder.capacity() >= 1_000_000);
}

#[test]
fn piece_by_piece() {
    const BLOCK_SIZE: usize = 65_536;
    const LEN_BYTES: BlockSize = 1000;

    // We grab a block of memory and split the single block into multiple views piece by piece.
    let memory = GlobalPool::new();

    let mut buf = memory.reserve(BLOCK_SIZE);

    let mut views = Vec::new();

    while buf.remaining_capacity() > 0 {
        #[expect(clippy::cast_possible_truncation, reason = "intentionally truncating")]
        let value = views.len() as u8;

        buf.put_byte_repeated(value, (LEN_BYTES as usize).min(buf.remaining_capacity()));

        // Sanity check against silly mutations.
        debug_assert!(!buf.is_empty());

        views.push(buf.consume_all());
    }

    #[expect(clippy::cast_possible_truncation, reason = "block size is small")]
    let expected_count = (BLOCK_SIZE as BlockSize).div_ceil(LEN_BYTES);
    assert_eq!(views.len(), expected_count as usize);

    assert!(!views.is_empty());

    for (i, sequence) in views.iter().enumerate() {
        #[expect(clippy::cast_possible_truncation, reason = "intentionally truncating")]
        let expected_value = i as u8;

        assert_eq!(sequence.first_slice()[0], expected_value);
    }
}

#[test]
#[cfg_attr(miri, ignore)] // Miri runtime scales with memory access size, so this takes forever.
fn large_content_survives_trip() {
    const SIZE_10MB: usize = 10 * 1024 * 1024;

    let pattern = testing_aids::repeating_incrementing_bytes().take(SIZE_10MB).collect::<Vec<_>>();

    let memory = GlobalPool::new();

    let mut buf = memory.reserve(SIZE_10MB);

    buf.put_slice(pattern.as_slice());

    let message = buf.consume_all();

    // Verify the sequence contains the expected pattern
    assert_eq!(message.len(), SIZE_10MB);
    assert_eq!(message, pattern.as_slice());
}

#[test]
#[cfg_attr(miri, ignore)] // Miri runtime scales with memory access size, so this takes forever.
fn two_large_views_different_patterns() {
    const SIZE_10MB: usize = 10 * 1024 * 1024;

    let pattern1 = testing_aids::repeating_incrementing_bytes().take(SIZE_10MB).collect::<Vec<_>>();

    let pattern2 = testing_aids::repeating_reverse_incrementing_bytes()
        .take(SIZE_10MB)
        .collect::<Vec<_>>();

    let memory = GlobalPool::new();

    // Create first sequence with ascending pattern
    let mut sb1 = memory.reserve(SIZE_10MB);
    sb1.put_slice(pattern1.as_slice());

    // Create second sequence with descending pattern
    let mut sb2 = memory.reserve(SIZE_10MB);
    sb2.put_slice(pattern2.as_slice());

    let view1 = sb1.consume_all();
    let view2 = sb2.consume_all();

    // Verify both views have correct size
    assert_eq!(view1.len(), SIZE_10MB);
    assert_eq!(view2.len(), SIZE_10MB);

    assert_eq!(view1, pattern1.as_slice());
    assert_eq!(view2, pattern2.as_slice());
}

#[test]
fn small_reservation_uses_1k_block() {
    let memory = GlobalPool::new();

    let buf = memory.reserve(500);
    assert_eq!(buf.capacity(), 1024);
}

#[test]
fn exact_1k_uses_1k_block() {
    let memory = GlobalPool::new();

    let buf = memory.reserve(1024);
    assert_eq!(buf.capacity(), 1024);
}

#[test]
fn just_over_1k_uses_4k_block() {
    let memory = GlobalPool::new();

    let buf = memory.reserve(1025);
    assert_eq!(buf.capacity(), 4096);
}

#[test]
fn exact_4k_uses_4k_block() {
    let memory = GlobalPool::new();

    let buf = memory.reserve(4096);
    assert_eq!(buf.capacity(), 4096);
}

#[test]
fn just_over_4k_uses_16k_block() {
    let memory = GlobalPool::new();

    let buf = memory.reserve(4097);
    assert_eq!(buf.capacity(), 16_384);
}

#[test]
fn exact_16k_uses_16k_block() {
    let memory = GlobalPool::new();

    let buf = memory.reserve(16_384);
    assert_eq!(buf.capacity(), 16_384);
}

#[test]
fn just_over_16k_uses_64k_block() {
    let memory = GlobalPool::new();

    let buf = memory.reserve(16_385);
    assert_eq!(buf.capacity(), 65_536);
}

#[test]
fn exact_64k_uses_64k_block() {
    let memory = GlobalPool::new();

    let buf = memory.reserve(65_536);
    assert_eq!(buf.capacity(), 65_536);
}

#[test]
fn multi_block_uniform_64k() {
    // 70KB exceeds 64KB, so uses multiple 64KB blocks (all same size).
    let memory = GlobalPool::new();

    let buf = memory.reserve(70_000);
    assert_eq!(buf.capacity(), 65_536 * 2);
}

#[test]
fn multi_block_many_64k() {
    // 200KB = ceil(200_000 / 65_536) = 4 blocks of 64KB.
    let memory = GlobalPool::new();

    let buf = memory.reserve(200_000);
    assert_eq!(buf.capacity(), 65_536 * 4);
}

#[test]
fn zero_reservation() {
    let memory = GlobalPool::new();

    let buf = memory.reserve(0);
    // Zero reservation should not panic and should result in zero capacity (no blocks).
    assert_eq!(buf.capacity(), 0);
}

#[test]
fn multi_block_just_over_64k() {
    // 66_036 > 64KB, so 2x 64KB blocks (uniform sizing).
    let memory = GlobalPool::new();

    let buf = memory.reserve(66_036);
    assert_eq!(buf.capacity(), 65_536 * 2);
}

#[test]
fn relocated_pool_works() {
    let mut memory = GlobalPool::new();

    // Allocate from the original pool.
    let mut buf = memory.reserve(100);
    buf.put_byte(42);
    let view = buf.consume_all();
    assert_eq!(view.first_slice()[0], 42);

    // Relocate the pool to a different thread coordinate.
    _ = Relocator::between_threads().relocate(&mut memory);

    // The relocated pool should work independently.
    let mut buf2 = memory.reserve(200);
    buf2.put_byte(99);
    let view2 = buf2.consume_all();
    assert_eq!(view2.first_slice()[0], 99);
}
