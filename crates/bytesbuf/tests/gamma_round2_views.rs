// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Byte-content oracles with unequal span boundaries.

use bytesbuf::BytesView;

fn segmented(data: &[u8], width: usize) -> BytesView {
    BytesView::from_views(data.chunks(width).map(|part| BytesView::from(part.to_vec())))
}

#[test]
fn gamma_round2_view_equality_checks_every_span_on_both_sides() {
    let data: Vec<u8> = (0..35).collect();
    for left_width in [1, 3, 7, 35] {
        for right_width in [1, 2, 5, 35] {
            let left = segmented(&data, left_width);
            assert_eq!(left, segmented(&data, right_width));
            for changed in 0..data.len() {
                let mut different = data.clone();
                different[changed] ^= 0x80;
                let right = segmented(&different, right_width);
                assert_ne!(left, right, "{left_width}/{right_width} byte {changed}");
                assert_ne!(right, left);
            }
        }
    }
}

#[test]
fn gamma_round2_append_nonempty_view_preserves_prefix_and_span_order() {
    let mut left = segmented(b"abcde", 2);
    left.append(segmented(b"FGHIJK", 3));
    assert_eq!(left.len(), 11);
    assert_eq!(left.to_vec(), b"abcdeFGHIJK");
    assert_eq!(
        left.slices().map(|(slice, _)| slice.to_vec()).collect::<Vec<_>>(),
        [b"ab".to_vec(), b"cd".to_vec(), b"e".to_vec(), b"FGH".to_vec(), b"IJK".to_vec()]
    );
}

#[cfg(feature = "seismograph")]
#[test]
fn gamma_nov4_append_reidentifies_combined_data_but_transfers_into_empty_buffers() {
    use bytesbuf::BytesBuf;
    use seismograph_io::Buffer;

    let first = BytesView::from(b"abc".to_vec());
    let first_id = first.recording_state().id().unwrap();
    let mut buffer = BytesBuf::new();
    buffer.put_bytes(first);
    assert_eq!(buffer.recording_state().id(), Some(first_id));
    buffer.put_bytes(BytesView::new());
    assert_eq!(buffer.recording_state().id(), Some(first_id));

    let second = BytesView::from(b"defg".to_vec());
    let second_id = second.recording_state().id().unwrap();
    buffer.put_bytes(second);
    let combined = buffer.recording_state();
    assert_ne!(combined.id(), Some(first_id));
    assert_ne!(combined.id(), Some(second_id));
    assert_eq!((combined.len(), combined.span_count()), (7, 2));
    assert_eq!(buffer.recording_state().id(), combined.id());

    let prefix = buffer.consume(3);
    assert_eq!(prefix.to_vec(), b"abc");
    assert_eq!(prefix.recording_state().id(), combined.id());
    assert_eq!(prefix.recording_state().span_count(), 1);
    let tail_id = buffer.recording_state().id();
    assert_ne!(tail_id, combined.id());
    let tail = buffer.consume_all();
    assert_eq!(tail.to_vec(), b"defg");
    assert_eq!(tail.recording_state().id(), tail_id);
    assert_eq!(tail.recording_state().span_count(), 1);
}

#[cfg(feature = "seismograph")]
#[test]
fn gamma_nov4_view_append_distinguishes_transfer_from_combination_identity() {
    use seismograph_io::Buffer;

    let mut left = BytesView::from(b"abc".to_vec());
    let left_id = left.recording_state().id();
    left.append(BytesView::new());
    assert_eq!(left.recording_state().id(), left_id);

    let right = BytesView::from(b"DE".to_vec());
    let right_id = right.recording_state().id();
    left.append(right);
    let combined = left.recording_state();
    assert_ne!(combined.id(), left_id);
    assert_ne!(combined.id(), right_id);
    assert_eq!((combined.len(), combined.span_count()), (5, 2));
    assert_eq!(left.to_vec(), b"abcDE");

    let mut empty = BytesView::new();
    empty.append(left);
    assert_eq!(empty.recording_state().id(), combined.id());
    assert_eq!(empty.to_vec(), b"abcDE");
}

#[test]
fn gamma_round2_inclusive_reversed_ranges_return_none() {
    use std::ops::Bound::{Excluded, Included};
    let view = segmented(b"abcdef", 2);
    assert!(view.range_checked((Included(4), Included(1))).is_none());
    assert!(view.range_checked((Excluded(5), Included(2))).is_none());
}

#[test]
fn gamma_round2_zero_advance_requires_no_writable_span() {
    let mut buffer = bytesbuf::BytesBuf::new();
    // SAFETY: Advancing zero bytes accesses no storage and initializes no bytes.
    unsafe { buffer.advance(0) };
    assert_eq!((buffer.len(), buffer.remaining_capacity()), (0, 0));
    buffer.put_bytes(BytesView::from(b"frozen".to_vec()));
    // SAFETY: The zero count accesses no memory even though all storage is frozen.
    unsafe { buffer.advance(0) };
    assert_eq!(buffer.peek().to_vec(), b"frozen");
    assert_eq!(buffer.remaining_capacity(), 0);
}

#[test]
fn gamma_round2_every_range_matches_contiguous_bytes_across_unequal_spans() {
    let data: Vec<u8> = (0..19).collect();
    for width in [1, 2, 5, 19] {
        let view = segmented(&data, width);
        for start in 0..=20 {
            for end in 0..=20 {
                let expected = data.get(start..end).map(<[u8]>::to_vec);
                assert_eq!(
                    view.range_checked(start..end).map(|range| range.to_vec()),
                    expected,
                    "width {width}, {start}..{end}"
                );
            }
        }
    }
}

#[cfg(feature = "test-util")]
#[test]
fn gamma_round2_spare_split_writes_disjoint_tail_bytes() {
    use std::num::NonZero;

    use bytesbuf::mem::testing::FixedBlockMemory;

    let memory = FixedBlockMemory::new(NonZero::new(11).unwrap());
    let mut buffer = memory.reserve(33);
    buffer.put_slice(*b"ABC");
    let spare_start = buffer.first_unfilled_slice().as_ptr().addr();
    let mut tail = buffer.split_off_remaining(25);
    assert_eq!((buffer.len(), buffer.remaining_capacity()), (3, 5));
    assert_eq!((tail.len(), tail.remaining_capacity()), (0, 25));
    assert_eq!(tail.first_unfilled_slice().as_ptr().addr(), spare_start + 5);
    tail.put_byte_repeated(0x71, 25);
    buffer.put_slice(*b"defgh");
    assert_eq!(buffer.peek().to_vec(), b"ABCdefgh");
    assert_eq!(tail.peek().to_vec(), vec![0x71; 25]);
}

#[cfg(feature = "test-util")]
#[test]
fn gamma_nov4_spare_split_stops_at_a_large_tail_before_a_smaller_builder() {
    use std::num::NonZero;

    use bytesbuf::mem::testing::FixedBlockMemory;

    let small = FixedBlockMemory::new(NonZero::new(5).unwrap());
    let large = FixedBlockMemory::new(NonZero::new(11).unwrap());
    let mut buffer = small.reserve(5);
    buffer.reserve(16, &large);
    assert_eq!(buffer.remaining_capacity(), 16);
    let mut tail = buffer.split_off_remaining(6);
    assert_eq!((buffer.remaining_capacity(), tail.remaining_capacity()), (10, 6));
    buffer.put_slice(*b"abcdefghij");
    tail.put_slice(*b"UVWXYZ");
    assert_eq!(buffer.peek().to_vec(), b"abcdefghij");
    assert_eq!(tail.peek().to_vec(), b"UVWXYZ");
    assert_eq!((buffer.remaining_capacity(), tail.remaining_capacity()), (0, 0));
}

#[cfg(feature = "bytes-compat")]
#[test]
fn gamma_round2_bytes_buf_copy_preserves_tail_and_empty_destination() {
    for width in [1, 3, 8] {
        let mut view = segmented(b"abcdefgh", width);
        bytes::Buf::copy_to_slice(&mut view, &mut []);
        assert_eq!(view.to_vec(), b"abcdefgh");
        let mut prefix = [0xff; 5];
        bytes::Buf::copy_to_slice(&mut view, &mut prefix);
        assert_eq!(&prefix, b"abcde");
        assert_eq!(view.to_vec(), b"fgh");
    }
}

#[cfg(feature = "bytes-compat")]
#[test]
fn gamma_round2_vectored_chunks_fill_exactly_the_available_output_slots() {
    for width in [1, 2, 5] {
        let view = segmented(b"abcdefghij", width);
        let chunks: Vec<_> = b"abcdefghij".chunks(width).collect();
        for capacity in 0..=12 {
            let mut output = vec![std::io::IoSlice::new(b"untouched"); capacity];
            let count = bytes::Buf::chunks_vectored(&view, &mut output);
            assert_eq!(count, capacity.min(chunks.len()));
            for index in 0..count {
                assert_eq!(&*output[index], chunks[index]);
            }
            for slot in &output[count..] {
                assert_eq!(&**slot, b"untouched");
            }
            assert_eq!(view.to_vec(), b"abcdefghij");
        }
    }
}
