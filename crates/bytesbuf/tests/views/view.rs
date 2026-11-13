// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Tests behavior through the public API.

#![allow(
    clippy::indexing_slicing,
    clippy::needless_range_loop,
    clippy::arithmetic_side_effects,
    reason = "This is all fine in test code"
)]

use std::hash::{Hash, Hasher};
use std::ops::Bound;
use std::pin::pin;
use std::thread;

use bytesbuf::mem::BlockMeta;
use bytesbuf::{BytesBuf, BytesView};
use new_zealand::nz;
use static_assertions::{assert_impl_all, assert_not_impl_any};
use testing_aids::assert_panic;

use crate::test_block::TestMemoryBlock;
use crate::test_memory::TestMemory;

assert_impl_all!(BytesView: Send, Sync, Eq);
assert_not_impl_any!(BytesView: From<&'static [u8]>);

/// A `Hasher` for which `write(a); write(b)` differs from `write(a ++ b)`, because it mixes the
/// length of every individual `write` call into the state. A correct [`Hash`] implementation
/// must not depend on how the bytes are chunked across `write` calls, so it must produce the
/// same hash for equal views regardless of their span segmentation even under such a hasher.
#[derive(Default)]
struct BoundarySensitiveHasher {
    state: u64,
}

impl Hasher for BoundarySensitiveHasher {
    fn finish(&self) -> u64 {
        self.state
    }

    fn write(&mut self, bytes: &[u8]) {
        self.state = self.state.wrapping_mul(31).wrapping_add(bytes.len() as u64);
        for &byte in bytes {
            self.state = self.state.wrapping_mul(31).wrapping_add(u64::from(byte));
        }
    }
}

/// A `Hasher` that records the exact bytes of every `write` call, so tests can assert that two
/// views drive the hasher with an identical sequence of calls rather than only comparing the
/// final (potentially colliding) hash value.
#[derive(Default)]
struct RecordingHasher {
    writes: Vec<Vec<u8>>,
}

impl Hasher for RecordingHasher {
    fn finish(&self) -> u64 {
        0
    }

    fn write(&mut self, bytes: &[u8]) {
        self.writes.push(bytes.to_vec());
    }
}

#[test]
fn smoke_test() {
    let mut span_builder = TestMemory::new().reserve(10);

    span_builder.put_slice(1234_u64.to_ne_bytes());
    span_builder.put_slice(16_u16.to_ne_bytes());

    let span1 = span_builder.consume(4);
    let span2 = span_builder.consume(3);
    let span3 = span_builder.consume(3);

    assert_eq!(0, span_builder.remaining_capacity());
    assert_eq!(span1.len(), 4);
    assert_eq!(span2.len(), 3);
    assert_eq!(span3.len(), 3);

    let mut view = BytesView::from_views(vec![span1, span2, span3]);

    assert!(!view.is_empty());
    assert_eq!(10, view.len());

    let slice = view.first_slice();
    assert_eq!(4, slice.len());

    // We read 8 bytes here, so should land straight inside span3.
    assert_eq!(view.get_u64_ne(), 1234);

    assert_eq!(2, view.len());

    let slice = view.first_slice();
    assert_eq!(2, slice.len());

    assert_eq!(view.get_u16_ne(), 16);

    assert_eq!(0, view.len());
    assert!(view.is_empty());
}

#[test]
fn oob_is_panic() {
    let mut span_builder = TestMemory::new().reserve(10);

    span_builder.put_slice(1234_u64.to_ne_bytes());
    span_builder.put_slice(16_u16.to_ne_bytes());

    let span1 = span_builder.consume(4);
    let span2 = span_builder.consume(3);
    let span3 = span_builder.consume(3);

    let mut view = BytesView::from_views(vec![span1, span2, span3]);

    assert_eq!(10, view.len());

    assert_eq!(view.get_u64_ne(), 1234);
    assert_panic!(_ = view.get_u32_ne()); // Reads 4 but only has 2 remaining.
}

#[test]
fn extend_lifetime_references_all_blocks() {
    // We need to detect here whether a block is being released (i.e. ref count goes to zero).

    // SAFETY: We are not allowed to drop this until all BlockRef are gone. This is fine
    // because it is dropped at the end of the function, after all BlockRef instances.
    let block1 = unsafe { TestMemoryBlock::new(nz!(8), None) };
    let block1 = pin!(block1);

    // SAFETY: We are not allowed to drop this until all BlockRef are gone. This is fine
    // because it is dropped at the end of the function, after all BlockRef instances.
    let block2 = unsafe { TestMemoryBlock::new(nz!(8), None) };
    let block2 = pin!(block2);

    let guard = {
        // SAFETY: We guarantee exclusive access to the memory capacity.
        let mut span_builder1 = BytesBuf::from_blocks([unsafe { block1.as_ref().to_block() }]);
        // SAFETY: We guarantee exclusive access to the memory capacity.
        let mut span_builder2 = BytesBuf::from_blocks([unsafe { block2.as_ref().to_block() }]);

        span_builder1.put_slice(1234_u64.to_ne_bytes());
        span_builder2.put_slice(1234_u64.to_ne_bytes());

        let span1 = span_builder1.consume(8);
        let span2 = span_builder2.consume(8);

        let view = BytesView::from_views(vec![span1, span2]);

        view.extend_lifetime()
    };

    // The sequence was destroyed and all BlockRefs it was holding are gone.
    // However, the lifetime guard is still alive and has a BlockRef.

    assert_eq!(block1.ref_count(), 1);
    assert_eq!(block2.ref_count(), 1);

    drop(guard);

    // And now they should all be dead.
    assert_eq!(block1.ref_count(), 0);
    assert_eq!(block2.ref_count(), 0);
}

#[test]
fn from_views() {
    let mut span_builder = TestMemory::new().reserve(100);

    span_builder.put_slice(1234_u64.to_ne_bytes());
    span_builder.put_slice(5678_u64.to_ne_bytes());

    let span1 = span_builder.consume(8);
    let span2 = span_builder.consume(8);

    let view1 = BytesView::from_views(vec![span1]);
    let view2 = BytesView::from_views(vec![span2]);

    let mut combined_view = BytesView::from_views(vec![view1, view2]);

    assert_eq!(16, combined_view.len());

    assert_eq!(combined_view.get_u64_ne(), 1234);
    assert_eq!(combined_view.get_u64_ne(), 5678);
}

#[test]
fn empty_view() {
    let view = BytesView::default();

    assert!(view.is_empty());
    assert_eq!(0, view.len());
    assert_eq!(0, view.first_slice().len());
}

#[test]
fn slice_from_single_span_view() {
    // A very simple view to start with, consisting of just one 100 byte span.
    let span_builder = TestMemory::new().reserve(100);

    let mut buf = span_builder;

    for i in 0..100 {
        buf.put_byte(i);
    }

    let view = buf.consume_all();

    let mut sliced_view = view.range(50..55);

    assert_eq!(5, sliced_view.len());
    assert_eq!(100, view.len());

    assert_eq!(50, sliced_view.get_byte());

    assert_eq!(4, sliced_view.len());
    assert_eq!(100, view.len());

    assert_eq!(51, sliced_view.get_byte());
    assert_eq!(52, sliced_view.get_byte());
    assert_eq!(53, sliced_view.get_byte());
    assert_eq!(54, sliced_view.get_byte());

    assert_eq!(0, sliced_view.len());

    assert!(view.range_checked(0..101).is_none());
    assert!(view.range_checked(100..101).is_none());
    assert!(view.range_checked(101..101).is_none());
}

#[test]
fn slice_from_multi_span_view() {
    let memory = TestMemory::with_block_size(nz!(10));
    let mut buf = memory.reserve(100);

    for i in 0..100 {
        buf.put_byte(i);
    }

    let view = buf.consume_all();

    let mut first5 = view.range(0..5);
    assert_eq!(5, first5.len());
    assert_eq!(100, view.len());
    assert_eq!(0, first5.get_byte());

    let mut last5 = view.range(95..100);
    assert_eq!(5, last5.len());
    assert_eq!(100, view.len());
    assert_eq!(95, last5.get_byte());

    let mut middle5 = view.range(49..54);
    assert_eq!(5, middle5.len());
    assert_eq!(100, view.len());
    assert_eq!(49, middle5.get_byte());
    assert_eq!(50, middle5.get_byte());
    assert_eq!(51, middle5.get_byte());
    assert_eq!(52, middle5.get_byte());
    assert_eq!(53, middle5.get_byte());

    assert!(view.range_checked(0..101).is_none());
    assert!(view.range_checked(100..101).is_none());
    assert!(view.range_checked(101..101).is_none());
}

#[test]
fn slice_indexing_kinds() {
    let span_builder = TestMemory::new().reserve(10);

    let mut buf = span_builder;
    buf.put_byte(0);
    buf.put_byte(1);
    buf.put_byte(2);
    buf.put_byte(3);
    buf.put_byte(4);
    buf.put_byte(5);

    let data = buf.consume_all();

    let mut middle_four = data.range(1..5);
    assert_eq!(4, middle_four.len());
    assert_eq!(1, middle_four.get_byte());
    assert_eq!(2, middle_four.get_byte());
    assert_eq!(3, middle_four.get_byte());
    assert_eq!(4, middle_four.get_byte());

    let mut middle_four = data.range(1..=4);
    assert_eq!(4, middle_four.len());
    assert_eq!(1, middle_four.get_byte());
    assert_eq!(2, middle_four.get_byte());
    assert_eq!(3, middle_four.get_byte());
    assert_eq!(4, middle_four.get_byte());

    let mut last_two = data.range(4..);
    assert_eq!(2, last_two.len());
    assert_eq!(4, last_two.get_byte());
    assert_eq!(5, last_two.get_byte());

    let mut first_two = data.range(..2);
    assert_eq!(2, first_two.len());
    assert_eq!(0, first_two.get_byte());
    assert_eq!(1, first_two.get_byte());

    let mut first_two = data.range(..=1);
    assert_eq!(2, first_two.len());
    assert_eq!(0, first_two.get_byte());
    assert_eq!(1, first_two.get_byte());
}

#[test]
fn slice_checked_with_excluded_start_bound() {
    let span_builder = TestMemory::new().reserve(100);

    let mut buf = span_builder;
    buf.put_byte(0);
    buf.put_byte(1);
    buf.put_byte(2);
    buf.put_byte(3);
    buf.put_byte(4);
    buf.put_byte(5);
    buf.put_byte(6);
    buf.put_byte(7);
    buf.put_byte(8);

    let view = buf.consume_all();

    // Test with excluded start bound: (Bound::Excluded(1), Bound::Excluded(5))
    // This should be equivalent to 2..5 (items at indices 2, 3, 4)
    let sliced = view.range_checked((Bound::Excluded(1), Bound::Excluded(5)));
    assert!(sliced.is_some());
    let mut sliced = sliced.unwrap();
    assert_eq!(3, sliced.len());
    assert_eq!(2, sliced.get_byte());
    assert_eq!(3, sliced.get_byte());
    assert_eq!(4, sliced.get_byte());

    // Test edge case: excluded start at the last valid index returns empty sequence
    let sliced = view.range_checked((Bound::Excluded(8), Bound::Unbounded));
    assert!(sliced.is_some());
    assert_eq!(0, sliced.unwrap().len());

    // Test edge case: excluded start that would overflow when adding 1
    let sliced = view.range_checked((Bound::Excluded(usize::MAX), Bound::Unbounded));
    assert!(sliced.is_none());
}

#[test]
fn slice_oob_is_panic() {
    let span_builder = TestMemory::new().reserve(1000);

    let mut buf = span_builder;
    buf.put_byte_repeated(0, 100);

    let view = buf.consume_all();

    assert_panic!(_ = view.range(0..101));
    assert_panic!(_ = view.range(0..=100));
    assert_panic!(_ = view.range(100..=100));
    assert_panic!(_ = view.range(100..101));
    assert_panic!(_ = view.range(101..));
    assert_panic!(_ = view.range(101..101));
    assert_panic!(_ = view.range(101..=101));
}

#[test]
fn slice_at_boundary_is_not_panic() {
    let span_builder = TestMemory::new().reserve(100);

    let mut buf = span_builder;
    buf.put_byte_repeated(0, 100);

    let view = buf.consume_all();

    assert_eq!(0, view.range(0..0).len());
    assert_eq!(1, view.range(0..=0).len());
    assert_eq!(0, view.range(..0).len());
    assert_eq!(1, view.range(..=0).len());
    assert_eq!(0, view.range(100..100).len());
    assert_eq!(0, view.range(99..99).len());
    assert_eq!(1, view.range(99..=99).len());
    assert_eq!(1, view.range(99..).len());
    assert_eq!(100, view.range(..).len());
}

#[test]
fn slice_empty_is_empty_if_not_oob() {
    let span_builder = TestMemory::new().reserve(100);

    let mut buf = span_builder;

    for i in 0..100 {
        buf.put_byte(i);
    }

    let view = buf.consume_all();

    let sub_view = view.range(50..50);
    assert_eq!(0, sub_view.len());

    // 100 is the index at the end of the view - still in-bounds, if at edge.
    let sub_view = view.range(100..100);
    assert_eq!(0, sub_view.len());
    assert!(view.range_checked(101..101).is_none());
}

#[test]
fn consume_all_slices() {
    let memory = TestMemory::with_block_size(nz!(10));
    let mut buf = memory.reserve(100);

    for i in 0..100 {
        buf.put_byte(i);
    }

    let mut view = buf.consume_all();

    let mut slice_index = 0;
    let mut bytes_consumed = 0;

    view.consume_all_slices(|slice| {
        assert_eq!(slice.len(), 10);
        bytes_consumed += slice.len();

        for i in 0..10 {
            assert_eq!(slice_index * 10 + i, slice[i] as usize);
        }

        slice_index += 1;
    });

    assert_eq!(bytes_consumed, 100);

    view.consume_all_slices(|_| unreachable!("view should now be empty"));
}

#[test]
fn multithreaded_usage() {
    fn post_to_another_thread(view: BytesView) {
        thread::spawn(move || {
            let mut view = view;
            assert_eq!(view.get_byte(), b'H');
            assert_eq!(view.get_byte(), b'e');
            assert_eq!(view.get_byte(), b'l');
            assert_eq!(view.get_byte(), b'l');
            assert_eq!(view.get_byte(), b'o');
        })
        .join()
        .unwrap();
    }

    let memory = TestMemory::new();
    let view = BytesView::copied_from_slice(b"Hello, world!", &memory);

    post_to_another_thread(view);
}

#[test]
fn slices_iterator() {
    let memory = TestMemory::new();
    let segment1 = BytesView::copied_from_slice(b"Hello, world!", &memory);
    let segment2 = BytesView::copied_from_slice(b"Hello, another world!", &memory);

    let view = BytesView::from_views(vec![segment1.clone(), segment2.clone()]);

    let slices: Vec<_> = view.slices().collect();

    assert_eq!(slices.len(), 2);
    assert_eq!(slices[0].0.len(), segment1.len());
    assert_eq!(slices[1].0.len(), segment2.len());
}

#[test]
fn slices_iterator_empty() {
    let view = BytesView::new();
    assert_eq!(view.slices().count(), 0);
}

#[test]
fn to_vec_single_span() {
    let memory = TestMemory::new();
    let view = BytesView::copied_from_slice(b"Hello, world!", &memory);

    let bytes = view.to_vec();

    assert_eq!(bytes, b"Hello, world!");
    // The view is left unchanged.
    assert_eq!(view.len(), 13);
}

#[test]
fn to_vec_multi_span() {
    let memory = TestMemory::new();
    let segment1 = BytesView::copied_from_slice(b"Hello, ", &memory);
    let segment2 = BytesView::copied_from_slice(b"world!", &memory);

    let view = BytesView::from_views(vec![segment1, segment2]);

    let bytes = view.to_vec();

    assert_eq!(bytes, b"Hello, world!");
}

#[test]
fn to_vec_empty() {
    let view = BytesView::new();

    let bytes = view.to_vec();

    assert!(bytes.is_empty());
}

#[test]
fn eq_view() {
    let memory = TestMemory::new();

    let view1 = BytesView::copied_from_slice(b"Hello, world!", &memory);
    let view2 = BytesView::copied_from_slice(b"Hello, world!", &memory);

    assert_eq!(view1, view2);

    let view3 = BytesView::copied_from_slice(b"Jello, world!", &memory);

    assert_ne!(view1, view3);

    let view4 = BytesView::copied_from_slice(b"Hello, world! ", &memory);

    assert_ne!(view1, view4);

    let view5_part1 = BytesView::copied_from_slice(b"Hello, ", &memory);
    let view5_part2 = BytesView::copied_from_slice(b"world!", &memory);
    let view5 = BytesView::from_views([view5_part1, view5_part2]);

    assert_eq!(view1, view5);
    assert_ne!(view5, view3);

    let view6 = BytesView::copied_from_slice(b"Hello, ", &memory);

    assert_ne!(view1, view6);
    assert_ne!(view5, view6);

    // Compare two multi-span views with misaligned span boundaries to exercise
    // span index advancement on both sides of the comparison.
    let view7_part1 = BytesView::copied_from_slice(b"Hel", &memory);
    let view7_part2 = BytesView::copied_from_slice(b"lo, world!", &memory);
    let view7 = BytesView::from_views([view7_part1, view7_part2]);

    assert_eq!(view5, view7);
    assert_eq!(view7, view5);
}

#[test]
fn eq_slice() {
    let memory = TestMemory::new();

    let view1 = BytesView::copied_from_slice(b"Hello, world!", &memory);

    assert_eq!(view1, b"Hello, world!".as_slice());
    assert_ne!(view1, b"Jello, world!".as_slice());
    assert_ne!(view1, b"Hello, world! ".as_slice());

    assert_eq!(b"Hello, world!".as_slice(), view1);
    assert_ne!(b"Jello, world!".as_slice(), view1);
    assert_ne!(b"Hello, world! ".as_slice(), view1);

    let view2_part1 = BytesView::copied_from_slice(b"Hello, ", &memory);
    let view2_part2 = BytesView::copied_from_slice(b"world!", &memory);
    let view2 = BytesView::from_views([view2_part1, view2_part2]);

    assert_eq!(view2, b"Hello, world!".as_slice());
    assert_ne!(view2, b"Jello, world!".as_slice());
    assert_ne!(view2, b"Hello, world! ".as_slice());
    assert_ne!(view2, b"Hello, ".as_slice());

    assert_eq!(b"Hello, world!".as_slice(), view2);
    assert_ne!(b"Jello, world!".as_slice(), view2);
    assert_ne!(b"Hello, world! ".as_slice(), view2);
    assert_ne!(b"Hello, ".as_slice(), view2);
}

#[test]
fn eq_array() {
    let memory = TestMemory::new();

    let view1 = BytesView::copied_from_slice(b"Hello, world!", &memory);

    assert_eq!(view1, b"Hello, world!");
    assert_ne!(view1, b"Jello, world!");
    assert_ne!(view1, b"Hello, world! ");

    assert_eq!(b"Hello, world!", view1);
    assert_ne!(b"Jello, world!", view1);
    assert_ne!(b"Hello, world! ", view1);

    let view2_part1 = BytesView::copied_from_slice(b"Hello, ", &memory);
    let view2_part2 = BytesView::copied_from_slice(b"world!", &memory);
    let view2 = BytesView::from_views([view2_part1, view2_part2]);

    assert_eq!(view2, b"Hello, world!");
    assert_ne!(view2, b"Jello, world!");
    assert_ne!(view2, b"Hello, world! ");
    assert_ne!(view2, b"Hello, ");

    assert_eq!(b"Hello, world!", view2);
    assert_ne!(b"Jello, world!", view2);
    assert_ne!(b"Hello, world! ", view2);
    assert_ne!(b"Hello, ", view2);
}

#[test]
fn first_slice_meta_empty_view() {
    let view = BytesView::new();
    assert!(view.first_slice_meta().is_none());
}

#[test]
fn first_slice_meta_no_metadata() {
    let memory = TestMemory::new();
    let view = BytesView::copied_from_slice(b"Hello", &memory);
    // TestMemory does not attach metadata.
    assert!(view.first_slice_meta().is_none());
}

#[test]
fn first_slice_meta_with_metadata() {
    #[derive(Debug)]
    struct TestMeta;
    impl BlockMeta for TestMeta {}

    // SAFETY: We are not allowed to drop this until all BlockRef are gone. This is fine
    // because it is dropped at the end of the function, after all BlockRef instances.
    let block = unsafe { TestMemoryBlock::new(nz!(100), Some(Box::new(TestMeta))) };
    let block = pin!(block);

    // SAFETY: We guarantee exclusive access to the memory capacity.
    let mut span_builder = BytesBuf::from_blocks([unsafe { block.as_ref().to_block() }]);
    span_builder.put_slice(*b"Hello");
    let span = span_builder.consume(5);

    let view = BytesView::from_views(vec![span]);

    let meta = view.first_slice_meta();
    assert!(meta.is_some());
    assert!(meta.unwrap().is::<TestMeta>());
}

#[test]
fn meta_none() {
    let memory = TestMemory::new();

    let view1 = BytesView::copied_from_slice(b"Hello, ", &memory);
    let view2 = BytesView::copied_from_slice(b"world!", &memory);

    let view = BytesView::from_views([view1, view2]);

    let mut slices_iter = view.slices();

    // We have two chunks, both without metadata.
    let (data1, meta1) = slices_iter.next().expect("should have first slice");
    assert!(!data1.is_empty());
    assert!(meta1.is_none());

    let (data2, meta2) = slices_iter.next().expect("should have second slice");
    assert!(!data2.is_empty());
    assert!(meta2.is_none());

    assert!(slices_iter.next().is_none());
}

#[test]
fn meta_some() {
    #[derive(Debug)]
    struct GreenMeta;
    #[derive(Debug)]
    struct BlueMeta;

    impl BlockMeta for GreenMeta {}
    impl BlockMeta for BlueMeta {}

    // SAFETY: We are not allowed to drop this until all BlockRef are gone. This is fine
    // because it is dropped at the end of the function, after all BlockRef instances.
    let block1 = unsafe { TestMemoryBlock::new(nz!(100), Some(Box::new(GreenMeta {}))) };
    let block1 = pin!(block1);

    // SAFETY: We are not allowed to drop this until all BlockRef are gone. This is fine
    // because it is dropped at the end of the function, after all BlockRef instances.
    let block2 = unsafe { TestMemoryBlock::new(nz!(100), Some(Box::new(BlueMeta {}))) };
    let block2 = pin!(block2);

    // SAFETY: We guarantee exclusive access to the memory capacity.
    let block1 = unsafe { block1.as_ref().to_block() };
    // SAFETY: We guarantee exclusive access to the memory capacity.
    let block2 = unsafe { block2.as_ref().to_block() };

    let mut buf = BytesBuf::from_blocks([block1, block2]);

    // Add enough bytes to make use of both blocks.
    buf.put_byte_repeated(123, 166);

    let view = buf.consume_all();

    let mut slices_iter = view.slices();

    // NB! There is no requirement that the BytesBuf use the blocks in the order we gave
    // them in. We use white-box knowledge here to know that it actually reverses the order.
    // This behavior may change in a future version - be ready to change the test if so.

    let (data1, meta1) = slices_iter.next().expect("should have first block");
    assert!(!data1.is_empty());
    assert!(meta1.is_some());
    assert!(meta1.unwrap().is::<BlueMeta>());
    assert!(!meta1.unwrap().is::<GreenMeta>());

    let (data2, meta2) = slices_iter.next().expect("should have second block");
    assert!(!data2.is_empty());
    assert!(meta2.is_some());
    assert!(meta2.unwrap().is::<GreenMeta>());
    assert!(!meta2.unwrap().is::<BlueMeta>());

    assert!(slices_iter.next().is_none(), "should have no more slices");
}

#[test]
fn append_single_span() {
    let memory = TestMemory::new();

    // Create two single-span views.
    let mut view1 = BytesView::copied_from_slice(b"Hello, ", &memory);
    let view2 = BytesView::copied_from_slice(b"world!", &memory);

    assert_eq!(view1.len(), 7);
    assert_eq!(view2.len(), 6);

    view1.append(view2);

    assert_eq!(view1.len(), 13);
    assert_eq!(view1, b"Hello, world!");
}

#[test]
fn append_multi_span() {
    let memory = TestMemory::new();

    // Create two multi-span views (2 spans each)
    let view1_part1 = BytesView::copied_from_slice(b"AAA", &memory);
    let view1_part2 = BytesView::copied_from_slice(b"BBB", &memory);
    let mut view1 = BytesView::from_views([view1_part1, view1_part2]);

    let view2_part1 = BytesView::copied_from_slice(b"CCC", &memory);
    let view2_part2 = BytesView::copied_from_slice(b"DDD", &memory);
    let view2 = BytesView::from_views([view2_part1, view2_part2]);

    assert_eq!(view1.len(), 6);
    assert_eq!(view2.len(), 6);

    view1.append(view2);

    assert_eq!(view1.len(), 12);
    assert_eq!(view1, b"AAABBBCCCDDD");
}

#[test]
fn append_empty_view() {
    let memory = TestMemory::new();

    let mut view1 = BytesView::copied_from_slice(b"Hello", &memory);
    let view2 = BytesView::new();

    view1.append(view2);
    assert_eq!(view1.len(), 5);
    assert_eq!(view1, b"Hello");

    let mut view3 = BytesView::new();
    let view4 = BytesView::copied_from_slice(b"world", &memory);

    view3.append(view4);
    assert_eq!(view3.len(), 5);
    assert_eq!(view3, b"world");
}

#[cfg(feature = "seismograph")]
#[test]
fn append_preserves_identity_when_only_one_view_has_data() {
    use seismograph_io::Buffer;
    let memory = TestMemory::new();
    let mut populated = BytesView::copied_from_slice(b"Hello", &memory);
    let populated_id = populated.recording_state().id();
    populated.append(BytesView::new());

    let mut empty = BytesView::new();
    let transferred = BytesView::copied_from_slice(b"world", &memory);
    let transferred_id = transferred.recording_state().id();
    empty.append(transferred);

    assert_eq!(
        (populated.recording_state().id(), empty.recording_state().id()),
        (populated_id, transferred_id)
    );
}

#[test]
fn concat_single_span() {
    let memory = TestMemory::new();

    // Create two single-span views
    let view1 = BytesView::copied_from_slice(b"Hello, ", &memory);
    let view2 = BytesView::copied_from_slice(b"world!", &memory);

    assert_eq!(view1.len(), 7);
    assert_eq!(view2.len(), 6);

    let view3 = view1.concat(view2);

    // Original view unchanged
    assert_eq!(view1.len(), 7);
    assert_eq!(view1, b"Hello, ");

    // New view contains combined data
    assert_eq!(view3.len(), 13);
    assert_eq!(view3, b"Hello, world!");
}

#[test]
fn concat_multi_span() {
    let memory = TestMemory::new();

    // Create two multi-span views (2 spans each)
    let view1_part1 = BytesView::copied_from_slice(b"AAA", &memory);
    let view1_part2 = BytesView::copied_from_slice(b"BBB", &memory);
    let view1 = BytesView::from_views([view1_part1, view1_part2]);

    let view2_part1 = BytesView::copied_from_slice(b"CCC", &memory);
    let view2_part2 = BytesView::copied_from_slice(b"DDD", &memory);
    let view2 = BytesView::from_views([view2_part1, view2_part2]);

    assert_eq!(view1.len(), 6);
    assert_eq!(view2.len(), 6);

    let view3 = view1.concat(view2);

    // Original view unchanged
    assert_eq!(view1.len(), 6);
    assert_eq!(view1, b"AAABBB");

    // New view contains combined data
    assert_eq!(view3.len(), 12);
    assert_eq!(view3, b"AAABBBCCCDDD");
}

#[test]
fn concat_empty_views() {
    let memory = TestMemory::new();

    let view1 = BytesView::copied_from_slice(b"Hello", &memory);
    let view2 = BytesView::new();

    let view3 = view1.concat(view2);
    assert_eq!(view3.len(), 5);
    assert_eq!(view3, b"Hello");

    let view4 = BytesView::new();
    let view5 = BytesView::copied_from_slice(b"world", &memory);

    let view6 = view4.concat(view5);
    assert_eq!(view6.len(), 5);
    assert_eq!(view6, b"world");
}

#[test]
fn hash_equal_views_produce_same_hash() {
    use std::hash::{DefaultHasher, Hash, Hasher};

    let a = BytesView::from(vec![1, 2, 3, 4, 5]);
    let b = BytesView::from(vec![1, 2, 3, 4, 5]);

    let hash_of = |v: &BytesView| {
        let mut h = DefaultHasher::new();
        v.hash(&mut h);
        h.finish()
    };

    assert_eq!(hash_of(&a), hash_of(&b));
}

#[test]
fn hash_multi_span_matches_single_span() {
    use std::hash::{DefaultHasher, Hash, Hasher};

    let single = BytesView::from(vec![1, 2, 3, 4, 5, 6]);

    let mut multi = BytesView::from(vec![1, 2, 3]);
    multi.append(BytesView::from(vec![4, 5, 6]));

    let hash_of = |v: &BytesView| {
        let mut h = DefaultHasher::new();
        v.hash(&mut h);
        h.finish()
    };

    assert_eq!(single, multi);
    assert_eq!(hash_of(&single), hash_of(&multi));
}

#[test]
fn hash_different_views_produce_different_hash() {
    use std::hash::{DefaultHasher, Hash, Hasher};

    let a = BytesView::from(vec![1, 2, 3]);
    let b = BytesView::from(vec![4, 5, 6]);

    let hash_of = |v: &BytesView| {
        let mut h = DefaultHasher::new();
        v.hash(&mut h);
        h.finish()
    };

    assert_ne!(hash_of(&a), hash_of(&b));
}

#[test]
fn hash_is_independent_of_span_segmentation() {
    // Same logical bytes, three different span segmentations: one, two, and three spans.
    let one_span = BytesView::from(vec![1, 2, 3, 4, 5, 6]);

    let mut two_spans = BytesView::from(vec![1, 2, 3]);
    two_spans.append(BytesView::from(vec![4, 5, 6]));

    let mut three_spans = BytesView::from(vec![1, 2]);
    three_spans.append(BytesView::from(vec![3, 4]));
    three_spans.append(BytesView::from(vec![5, 6]));

    // All three are equal per the boundary-independent equality.
    assert_eq!(one_span, two_spans);
    assert_eq!(one_span, three_spans);

    let hash_of = |v: &BytesView| {
        let mut h = BoundarySensitiveHasher::default();
        v.hash(&mut h);
        h.finish()
    };

    // The Hash/Eq contract requires equal values to hash equally, even under a hasher that is
    // sensitive to how bytes are chunked across write() calls.
    assert_eq!(hash_of(&one_span), hash_of(&two_spans));
    assert_eq!(hash_of(&one_span), hash_of(&three_spans));
}

#[test]
fn hash_is_independent_of_span_segmentation_across_block_boundary() {
    // Content longer than 64 so hashing spills across multiple blocks, with span
    // splits that do not line up with the block boundaries. The hash must still ignore how the
    // bytes are segmented into spans.
    let content: Vec<u8> = (0..=u8::MAX).cycle().take(64 * 2 + 5).collect();

    let one_span = BytesView::from(content.clone());

    // Split at offsets that straddle the 64-byte block boundary (before, on, and after it).
    let split_points = [1, 64 - 3, 64, 64 + 7];
    let mut many_spans = BytesView::new();
    let mut previous = 0;
    for &point in &split_points {
        many_spans.append(BytesView::from(content[previous..point].to_vec()));
        previous = point;
    }
    many_spans.append(BytesView::from(content[previous..].to_vec()));

    assert_eq!(one_span, many_spans);

    let hash_of = |v: &BytesView| {
        let mut h = BoundarySensitiveHasher::default();
        v.hash(&mut h);
        h.finish()
    };

    assert_eq!(hash_of(&one_span), hash_of(&many_spans));
}

#[test]
fn hash_write_sequence_is_identical_regardless_of_segmentation() {
    // Splits `content` into spans of at most `segment_len` bytes each.
    fn segmented_view(content: &[u8], segment_len: usize) -> BytesView {
        let mut view = BytesView::new();
        for chunk in content.chunks(segment_len) {
            view.append(BytesView::from(chunk.to_vec()));
        }
        view
    }

    let record = |v: &BytesView| {
        let mut h = RecordingHasher::default();
        v.hash(&mut h);
        h.writes
    };

    // Cover lengths around the block boundary (64 == 64) and beyond.
    for len in [0_usize, 1, 63, 64, 65, 128, 130, 200] {
        let content: Vec<u8> = (0..=u8::MAX).cycle().take(len).collect();

        let single = BytesView::from(content.clone());
        let baseline = record(&single);

        // Pin the absolute write sequence: the length prefix followed by the content split
        // into 64 chunks, with no trailing empty write (`chunks` never yields an
        // empty slice, including for an empty view). Asserting the exact sequence - rather than
        // only comparing segmentations against each other - is what catches a spurious empty
        // trailing write when the length is an exact multiple of the block size.
        let mut expected: Vec<Vec<u8>> = vec![len.to_ne_bytes().to_vec()];
        expected.extend(content.chunks(64).map(<[u8]>::to_vec));
        assert_eq!(baseline, expected, "unexpected hash write sequence for len {len}");

        for segment_len in [1, 7, 63, 64, 65, 100] {
            let segmented = segmented_view(&content, segment_len);

            assert_eq!(single, segmented, "len {len}, segment {segment_len}");
            assert_eq!(
                record(&segmented),
                baseline,
                "hash write sequence differs for len {len}, segment {segment_len}",
            );
        }
    }
}
