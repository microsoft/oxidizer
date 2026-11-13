// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Tests behavior through the public API.

#![allow(clippy::indexing_slicing, reason = "Fine in test code, we prefer panic on error")]

use std::mem::MaybeUninit;
use std::pin::pin;

#[cfg(feature = "std")]
use bytesbuf::mem::GlobalPool;
use bytesbuf::mem::{BlockMeta, Memory};
use bytesbuf::{BytesBuf, BytesView};
use new_zealand::nz;
use static_assertions::assert_impl_all;
use testing_aids::assert_panic;

use crate::test_block::TestMemoryBlock;
use crate::test_memory::TestMemory;

const U64_SIZE: usize = size_of::<u64>();
const TWO_U64_SIZE: usize = size_of::<u64>() + size_of::<u64>();
const THREE_U64_SIZE: usize = size_of::<u64>() + size_of::<u64>() + size_of::<u64>();

assert_impl_all!(BytesBuf: Send, Sync);

fn write_copy_of_slice(dst: &mut [MaybeUninit<u8>], src: &[u8]) {
    assert!(dst.len() >= src.len());
    // SAFETY: The destination has enough capacity and does not overlap the source.
    unsafe {
        src.as_ptr().copy_to_nonoverlapping(dst.as_mut_ptr().cast(), src.len());
    }
}

// Compile time test
fn _can_use_in_dyn_traits(mem: &dyn Memory) {
    let _ = mem.reserve(123);
    #[cfg(feature = "std")]
    let _ = BytesBuf::new().into_writer(mem);
}

#[cfg(feature = "seismograph")]
#[test]
fn seismograph_identity_follows_consumed_data() {
    use seismograph_io::Buffer;

    let memory = GlobalPool::new();
    let mut buffer = memory.reserve(16);
    buffer.put_slice(*b"abcd");
    let original_id = buffer.recording_state().id();

    let view = buffer.consume(2);
    let cloned = view.clone();

    assert_eq!(view.recording_state().id(), original_id);
    assert_ne!(buffer.recording_state().id(), original_id);
    assert_ne!(cloned.recording_state().id(), original_id);
    assert_ne!(view.range(..1).recording_state().id(), original_id);
}

#[cfg(feature = "seismograph")]
#[test]
fn seismograph_recording_state_reports_exact_buffer_shape() {
    use seismograph_io::Buffer;

    let memory = TestMemory::with_block_size(nz!(4));
    let mut buffer = memory.reserve(8);
    buffer.put_slice(*b"abcdef");

    let state = buffer.recording_state();
    assert_eq!((state.len(), state.span_count()), (6, 2));

    let empty = BytesBuf::new();
    let state = empty.recording_state();
    assert_eq!((state.len(), state.span_count()), (0, 0));
}

#[cfg(feature = "seismograph")]
#[test]
fn seismograph_io_recording_is_gated_and_captures_buffer_state_delta() {
    use seismograph::recorder::event::EventKind;
    use seismograph::recorder::{Configuration, EventBufferCapacity, Policy};
    use seismograph::snapshot::{EventBufferDisposition, SnapshotOptions};
    use seismograph_io::{Buffer, IoOutcome, IoResourceKind, Operation, Resource};

    let release = SnapshotOptions {
        event_buffers: EventBufferDisposition::Release,
    };
    let memory = TestMemory::with_block_size(nz!(4));
    let mut buffer = memory.reserve(8);
    buffer.put_slice(*b"abcdef");
    let resource = Resource::new(IoResourceKind::Other);

    seismograph::recorder(Configuration::default());
    seismograph::snapshot(release).unwrap();
    let disabled = Operation::write_started(&resource, 2, || buffer.recording_state());
    assert!(!disabled.was_recorded());
    disabled.finish(0, IoOutcome::Canceled, || buffer.recording_state());
    let snapshot = seismograph::snapshot(release).unwrap();
    let disabled_events = seismograph::snapshot::decode(snapshot.as_bytes()).unwrap().events;
    assert_eq!(
        (
            disabled_events.total_events,
            disabled_events.lost_events,
            disabled_events.threads.len(),
            disabled_events.events.len(),
        ),
        (0, 0, 0, 0)
    );

    seismograph::recorder(Configuration {
        io: Policy::all(false),
        event_capacity_per_thread: EventBufferCapacity::new(64),
        ..Configuration::default()
    });
    let initial_state = buffer.recording_state();
    let operation = Operation::write_started(&resource, 2, || buffer.recording_state());
    buffer.put_slice(*b"gh");
    operation.finish(2, IoOutcome::Success, || buffer.recording_state());

    let snapshot = seismograph::snapshot(release).unwrap();
    let events = seismograph::snapshot::decode(snapshot.as_bytes()).unwrap().events;
    let io = events
        .events
        .iter()
        .map(|event| (event.kind, event.io().unwrap()))
        .collect::<Vec<_>>();
    assert_eq!(
        (
            events.total_events,
            events.lost_events,
            events.threads.len(),
            io.iter()
                .map(|(kind, event)| (*kind, event.buffer_id, event.buffer_len, event.buffer_span_count))
                .collect::<Vec<_>>(),
        ),
        (
            2,
            0,
            1,
            vec![
                (
                    EventKind::IoWriteStarted,
                    initial_state.id(),
                    initial_state.len(),
                    initial_state.span_count(),
                ),
                (EventKind::IoWriteFinished, initial_state.id(), 8, 2),
            ],
        )
    );
    seismograph::recorder(Configuration::default());
}

#[test]
fn smoke_test() {
    let memory = TestMemory::with_block_size(nz!(1234));

    let min_length = 1000;

    let mut buf = memory.reserve(min_length);

    assert!(buf.capacity() >= min_length);
    assert!(buf.remaining_capacity() >= min_length);
    assert_eq!(buf.capacity(), buf.remaining_capacity());
    assert_eq!(buf.len(), 0);
    assert!(buf.is_empty());

    buf.put_u64_ne(1234);
    buf.put_u64_ne(5678);
    buf.put_u64_ne(1234);
    buf.put_u64_ne(5678);

    assert_eq!(buf.len(), 32);
    assert!(!buf.is_empty());

    // SAFETY: Writing 0 bytes is always valid.
    unsafe {
        buf.advance(0);
    }

    let mut first_two = buf.consume(TWO_U64_SIZE);
    let mut second_two = buf.consume(TWO_U64_SIZE);

    assert_eq!(first_two.len(), 16);
    assert_eq!(second_two.len(), 16);
    assert_eq!(buf.len(), 0);

    assert_eq!(first_two.get_u64_ne(), 1234);
    assert_eq!(first_two.get_u64_ne(), 5678);

    assert_eq!(second_two.get_u64_ne(), 1234);
    assert_eq!(second_two.get_u64_ne(), 5678);

    buf.put_u64_ne(1111);

    assert_eq!(buf.len(), 8);

    let mut last = buf.consume(U64_SIZE);

    assert_eq!(last.len(), 8);
    assert_eq!(buf.len(), 0);

    assert_eq!(last.get_u64_ne(), 1111);

    assert!(buf.consume_checked(1).is_none());
    assert!(buf.consume_all().is_empty());
}

#[test]
fn extend_capacity() {
    let mut buf = BytesBuf::new();

    assert_eq!(buf.capacity(), 0);
    assert_eq!(buf.remaining_capacity(), 0);

    let memory = TestMemory::with_block_size(nz!(100));

    // Have 0, desired 10, requesting 10, will get 100.
    buf.reserve(10, &memory);

    assert_eq!(buf.capacity(), 100);
    assert_eq!(buf.remaining_capacity(), 100);

    // Write 10 bytes of data just to verify that it does not affect "capacity" logic.
    buf.put_u64_ne(1234);
    buf.put_u16_ne(5678);

    assert_eq!(buf.len(), 10);
    assert_eq!(buf.remaining_capacity(), 90);
    assert_eq!(buf.capacity(), 100);

    // Have 100, desired 10+140=150, requesting 50, will get another 100 for a total of 200.
    buf.reserve(140, &memory);

    assert_eq!(buf.len(), 10);
    assert_eq!(buf.remaining_capacity(), 190);
    assert_eq!(buf.capacity(), 200);

    // Have 200, desired 10+200=210, 210-200=10, will get another 100.
    buf.reserve(200, &memory);

    assert_eq!(buf.len(), 10);
    assert_eq!(buf.remaining_capacity(), 290);
    assert_eq!(buf.capacity(), 300);
}

#[test]
fn append_existing_view() {
    let memory = TestMemory::with_block_size(nz!(1234));

    let min_length = 1000;

    // This one we use to prepare some data to append.
    let mut payload_buffer = memory.reserve(min_length);

    // This is where we append the data to.
    let mut target_buffer = memory.reserve(min_length);

    // First we make a couple pieces to append.
    payload_buffer.put_u64_ne(1111);
    payload_buffer.put_u64_ne(2222);
    payload_buffer.put_u64_ne(3333);
    payload_buffer.put_u64_ne(4444);

    let payload1 = payload_buffer.consume(TWO_U64_SIZE);
    let payload2 = payload_buffer.consume(TWO_U64_SIZE);

    // Then we prefill some data to start us off.
    target_buffer.put_u64_ne(5555);
    target_buffer.put_u64_ne(6666);

    // Consume a little just for extra complexity.
    let _ = target_buffer.consume(U64_SIZE);

    // Append the payloads.
    target_buffer.put_bytes(payload1);
    target_buffer.put_bytes(payload2);

    // Appending an empty byte sequence does nothing.
    target_buffer.put_bytes(BytesView::default());

    // Add some custom data at the end.
    target_buffer.put_u64_ne(7777);

    assert_eq!(target_buffer.len(), 48);

    let mut result = target_buffer.consume(48);

    assert_eq!(result.get_u64_ne(), 6666);
    assert_eq!(result.get_u64_ne(), 1111);
    assert_eq!(result.get_u64_ne(), 2222);
    assert_eq!(result.get_u64_ne(), 3333);
    assert_eq!(result.get_u64_ne(), 4444);
    assert_eq!(result.get_u64_ne(), 7777);
}

#[test]
fn consume_all_mixed() {
    let mut buf = BytesBuf::new();
    let memory = TestMemory::with_block_size(nz!(8));

    // Reserve some capacity and add initial data.
    buf.reserve(16, &memory);
    buf.put_u64_ne(1111);
    buf.put_u64_ne(2222);

    // Consume some data (the 1111).
    let _ = buf.consume(8);

    // Append a sequence (the 3333).
    let mut append_buf = BytesBuf::new();
    append_buf.reserve(8, &memory);
    append_buf.put_u64_ne(3333);
    let reused_bytes_to_append = append_buf.consume_all();
    buf.put_bytes(reused_bytes_to_append);

    // Add more data (the 4444).
    buf.reserve(8, &memory);
    buf.put_u64_ne(4444);

    // Consume all data and validate we got all the pieces.
    let mut result = buf.consume_all();

    assert_eq!(result.len(), 24);
    assert_eq!(result.get_u64_ne(), 2222);
    assert_eq!(result.get_u64_ne(), 3333);
    assert_eq!(result.get_u64_ne(), 4444);
}

#[test]
#[expect(clippy::cognitive_complexity, reason = "test code")]
fn peek_basic() {
    let mut buf = BytesBuf::new();

    assert_eq!(buf.capacity(), 0);
    assert_eq!(buf.remaining_capacity(), 0);

    let memory = TestMemory::with_block_size(nz!(10));

    // Peeking an empty buffer is fine, it is just an empty BytesView in that case.
    let peeked = buf.peek();
    assert_eq!(peeked.len(), 0);

    buf.reserve(100, &memory);

    assert_eq!(buf.capacity(), 100);

    buf.put_u64_ne(1111);

    // We have 0 frozen spans and 10 span builders,
    // the first of which has 8 bytes of filled content.
    let mut peeked = buf.peek();
    assert_eq!(peeked.first_slice().len(), 8);
    assert_eq!(peeked.get_u64_ne(), 1111);
    assert_eq!(peeked.len(), 0);

    buf.put_u64_ne(2222);
    buf.put_u64_ne(3333);
    buf.put_u64_ne(4444);
    buf.put_u64_ne(5555);
    buf.put_u64_ne(6666);
    buf.put_u64_ne(7777);
    buf.put_u64_ne(8888);
    // These will cross a span boundary so we can also observe
    // crossing that boundary during peeking.
    buf.put_byte_repeated(9, 8);

    assert_eq!(buf.len(), 72);
    assert_eq!(buf.capacity(), 100);
    assert_eq!(buf.remaining_capacity(), 28);

    // We should have 7 frozen spans and 3 span builders,
    // the first of which has 2 bytes of filled content.
    let mut peeked = buf.peek();

    assert_eq!(peeked.len(), 72);

    // This should be the first frozen span of 10 bytes.
    assert_eq!(peeked.first_slice().len(), 10);

    assert_eq!(peeked.get_u64_ne(), 1111);
    assert_eq!(peeked.get_u64_ne(), 2222);

    // The length of the buffer does not change just because we peek at its data.
    assert_eq!(buf.len(), 72);

    // We consumed 16 bytes from the peeked view, so should be looking at the remaining 4 bytes in the 2nd span.
    assert_eq!(peeked.first_slice().len(), 4);

    assert_eq!(peeked.get_u64_ne(), 3333);
    assert_eq!(peeked.get_u64_ne(), 4444);
    assert_eq!(peeked.get_u64_ne(), 5555);
    assert_eq!(peeked.get_u64_ne(), 6666);
    assert_eq!(peeked.get_u64_ne(), 7777);
    assert_eq!(peeked.get_u64_ne(), 8888);

    for _ in 0..8 {
        assert_eq!(peeked.get_byte(), 9);
    }

    assert_eq!(peeked.len(), 0);
    assert_eq!(peeked.first_slice().len(), 0);

    // Fill up the remaining 28 bytes of data so we have a full sequence builder.
    buf.put_byte_repeated(88, 28);

    let mut peeked = buf.peek();
    peeked.advance(72);

    assert_eq!(peeked.len(), 28);

    for _ in 0..28 {
        assert_eq!(peeked.get_byte(), 88);
    }
}

#[test]
fn consume_part_of_frozen_span() {
    let mut buf = BytesBuf::new();

    assert_eq!(buf.capacity(), 0);
    assert_eq!(buf.remaining_capacity(), 0);

    let memory = TestMemory::with_block_size(nz!(10));

    buf.reserve(100, &memory);

    assert_eq!(buf.capacity(), 100);

    buf.put_u64_ne(1111);
    // This freezes the first span of 10, as we filled it all up.
    buf.put_u64_ne(2222);

    let mut first8 = buf.consume(U64_SIZE);
    assert_eq!(first8.get_u64_ne(), 1111);
    assert!(first8.is_empty());

    buf.put_u64_ne(3333);

    let mut second16 = buf.consume(16);
    assert_eq!(second16.get_u64_ne(), 2222);
    assert_eq!(second16.get_u64_ne(), 3333);
    assert!(second16.is_empty());
}

#[test]
fn empty_buffer() {
    let mut buf = BytesBuf::new();
    assert!(buf.is_empty());
    assert!(buf.peek().is_empty());
    assert_eq!(0, buf.first_unfilled_slice().len());

    let consumed = buf.consume(0);
    assert!(consumed.is_empty());

    let consumed = buf.consume_all();
    assert!(consumed.is_empty());
}

#[test]
fn iter_available_empty_with_capacity() {
    let mut buf = BytesBuf::new();

    assert_eq!(buf.capacity(), 0);
    assert_eq!(buf.remaining_capacity(), 0);

    let memory = TestMemory::with_block_size(nz!(100));

    // Capacity: 0 -> 1000 (10x100)
    buf.reserve(1000, &memory);

    assert_eq!(buf.capacity(), 1000);
    assert_eq!(buf.remaining_capacity(), 1000);

    let mut write = buf.begin_vectored_write(None);
    let iter = write.slices_mut();

    // Demonstrating that we can access slices concurrently, not only one by one.
    let slices: Vec<_> = iter.map(|(s, _)| s).collect();

    assert_eq!(slices.len(), 10);

    for slice in slices {
        assert_eq!(slice.len(), 100);
    }

    // After we have dropped all slice references, it is again legal to access the buffer.
    // This is blocked by the borrow checker while slice references still exist.
    buf.reserve(100, &memory);
}

#[test]
fn iter_available_nonempty() {
    let mut buf = BytesBuf::new();

    assert_eq!(buf.capacity(), 0);
    assert_eq!(buf.remaining_capacity(), 0);

    let memory = TestMemory::with_block_size(nz!(8));

    // Capacity: 0 -> 16 (2x8)
    buf.reserve(TWO_U64_SIZE, &memory);

    assert_eq!(buf.capacity(), 16);
    assert_eq!(buf.remaining_capacity(), 16);

    // We write an u64 - this fills half the capacity and should result in
    // the first span builder being frozen and the second remaining in its entirety.
    buf.put_u64_ne(1234);

    assert_eq!(buf.len(), 8);
    assert_eq!(buf.remaining_capacity(), 8);

    let mut write = buf.begin_vectored_write(None);
    let available_slices: Vec<_> = write.slices_mut().map(|(s, _)| s).collect();
    assert_eq!(available_slices.len(), 1);
    assert_eq!(available_slices[0].len(), 8);

    // We write a u32 - this fills half the remaining capacity, which results
    // in a half-filled span builder remaining in the buffer.
    buf.put_u32_ne(5678);

    assert_eq!(buf.len(), 12);
    assert_eq!(buf.remaining_capacity(), 4);

    let mut write = buf.begin_vectored_write(None);
    let available_slices: Vec<_> = write.slices_mut().map(|(s, _)| s).collect();
    assert_eq!(available_slices.len(), 1);
    assert_eq!(available_slices[0].len(), 4);

    // We write a final u32 to use up all the capacity.
    buf.put_u32_ne(9012);

    assert_eq!(buf.len(), 16);
    assert_eq!(buf.remaining_capacity(), 0);

    assert_eq!(buf.begin_vectored_write(None).slices_mut().count(), 0);
}

#[test]
fn iter_available_empty_no_capacity() {
    let mut buf = BytesBuf::new();

    assert_eq!(buf.capacity(), 0);
    assert_eq!(buf.remaining_capacity(), 0);
    assert_eq!(buf.begin_vectored_write(None).slices_mut().count(), 0);
}

#[test]
fn vectored_write_zero() {
    let mut buf = BytesBuf::new();

    assert_eq!(buf.capacity(), 0);
    assert_eq!(buf.remaining_capacity(), 0);

    let memory = TestMemory::with_block_size(nz!(8));

    // Capacity: 0 -> 16 (2x8)
    buf.reserve(TWO_U64_SIZE, &memory);

    assert_eq!(buf.capacity(), 16);
    assert_eq!(buf.remaining_capacity(), 16);

    let vectored_write = buf.begin_vectored_write(None);

    // SAFETY: Yes, we really wrote 0 bytes.
    unsafe {
        vectored_write.commit(0);
    }

    assert_eq!(buf.capacity(), 16);
    assert_eq!(buf.remaining_capacity(), 16);
}

#[test]
fn vectored_write_one_slice() {
    let mut buf = BytesBuf::new();

    assert_eq!(buf.capacity(), 0);
    assert_eq!(buf.remaining_capacity(), 0);

    let memory = TestMemory::with_block_size(nz!(8));

    // Capacity: 0 -> 8 (1x8)
    buf.reserve(U64_SIZE, &memory);

    assert_eq!(buf.capacity(), 8);
    assert_eq!(buf.remaining_capacity(), 8);

    let mut vectored_write = buf.begin_vectored_write(None);

    let mut slices: Vec<_> = vectored_write.slices_mut().map(|(s, _)| s).collect();
    assert_eq!(slices.len(), 1);
    assert_eq!(slices[0].len(), 8);

    write_copy_of_slice(slices[0], &0x3333_3333_3333_3333_u64.to_ne_bytes());

    // SAFETY: Yes, we really wrote 8 bytes.
    unsafe {
        vectored_write.commit(8);
    }

    assert_eq!(buf.len(), 8);
    assert_eq!(buf.remaining_capacity(), 0);
    assert_eq!(buf.capacity(), 8);

    let mut result = buf.consume(U64_SIZE);
    assert_eq!(result.get_u64_ne(), 0x3333_3333_3333_3333);
}

#[test]
fn vectored_write_multiple_slices() {
    let mut buf = BytesBuf::new();

    assert_eq!(buf.capacity(), 0);
    assert_eq!(buf.remaining_capacity(), 0);

    let memory = TestMemory::with_block_size(nz!(8));

    // Capacity: 0 -> 24 (3x8)
    buf.reserve(THREE_U64_SIZE, &memory);

    assert_eq!(buf.capacity(), 24);
    assert_eq!(buf.remaining_capacity(), 24);

    let mut vectored_write = buf.begin_vectored_write(None);

    let mut slices: Vec<_> = vectored_write.slices_mut().map(|(s, _)| s).collect();
    assert_eq!(slices.len(), 3);
    assert_eq!(slices[0].len(), 8);
    assert_eq!(slices[1].len(), 8);
    assert_eq!(slices[2].len(), 8);

    // We fill 12 bytes, leaving middle chunk split in half between filled/available.

    write_copy_of_slice(slices[0], &0x3333_3333_3333_3333_u64.to_ne_bytes());
    write_copy_of_slice(slices[1], &0x4444_4444_u32.to_ne_bytes());

    // SAFETY: Yes, we really wrote 12 bytes.
    unsafe {
        vectored_write.commit(12);
    }

    assert_eq!(buf.len(), 12);
    assert_eq!(buf.remaining_capacity(), 12);
    assert_eq!(buf.capacity(), 24);

    let mut vectored_write = buf.begin_vectored_write(None);

    let mut slices: Vec<_> = vectored_write.slices_mut().map(|(s, _)| s).collect();
    assert_eq!(slices.len(), 2);
    assert_eq!(slices[0].len(), 4);
    assert_eq!(slices[1].len(), 8);

    // We fill the remaining 12 bytes.

    write_copy_of_slice(slices[0], &0x5555_5555_u32.to_ne_bytes());
    write_copy_of_slice(slices[1], &0x6666_6666_6666_6666_u64.to_ne_bytes());

    // SAFETY: Yes, we really wrote 12 bytes.
    unsafe {
        vectored_write.commit(12);
    }

    assert_eq!(buf.len(), 24);
    assert_eq!(buf.remaining_capacity(), 0);
    assert_eq!(buf.capacity(), 24);

    let mut result = buf.consume(THREE_U64_SIZE);
    assert_eq!(result.get_u64_ne(), 0x3333_3333_3333_3333);
    assert_eq!(result.get_u32_ne(), 0x4444_4444);
    assert_eq!(result.get_u32_ne(), 0x5555_5555);
    assert_eq!(result.get_u64_ne(), 0x6666_6666_6666_6666);
}

#[test]
fn vectored_write_max_len() {
    let mut buf = BytesBuf::new();

    assert_eq!(buf.capacity(), 0);
    assert_eq!(buf.remaining_capacity(), 0);

    let memory = TestMemory::with_block_size(nz!(8));

    // Capacity: 0 -> 24 (3x8)
    buf.reserve(THREE_U64_SIZE, &memory);

    assert_eq!(buf.capacity(), 24);
    assert_eq!(buf.remaining_capacity(), 24);

    // We limit to 13 bytes of visible capacity, of which we will fill 12.
    let mut vectored_write = buf.begin_vectored_write(Some(13));

    let mut slices: Vec<_> = vectored_write.slices_mut().map(|(s, _)| s).collect();
    assert_eq!(slices.len(), 2);
    assert_eq!(slices[0].len(), 8);
    assert_eq!(slices[1].len(), 5);

    // We fill 12 bytes, leaving middle chunk split in half between filled/available.

    write_copy_of_slice(slices[0], &0x3333_3333_3333_3333_u64.to_ne_bytes());
    write_copy_of_slice(slices[1], &0x4444_4444_u32.to_ne_bytes());

    // SAFETY: Yes, we really wrote 12 bytes.
    unsafe {
        vectored_write.commit(12);
    }

    assert_eq!(buf.len(), 12);
    assert_eq!(buf.remaining_capacity(), 12);
    assert_eq!(buf.capacity(), 24);

    // There are 12 remaining and we set max_limit to exactly cover those 12
    let mut vectored_write = buf.begin_vectored_write(Some(12));

    let mut slices: Vec<_> = vectored_write.slices_mut().map(|(s, _)| s).collect();
    assert_eq!(slices.len(), 2);
    assert_eq!(slices[0].len(), 4);
    assert_eq!(slices[1].len(), 8);

    write_copy_of_slice(slices[0], &0x5555_5555_u32.to_ne_bytes());
    write_copy_of_slice(slices[1], &0x6666_6666_6666_6666_u64.to_ne_bytes());

    // SAFETY: Yes, we really wrote 12 bytes.
    unsafe {
        vectored_write.commit(12);
    }

    assert_eq!(buf.len(), 24);
    assert_eq!(buf.remaining_capacity(), 0);
    assert_eq!(buf.capacity(), 24);

    let mut result = buf.consume(THREE_U64_SIZE);
    assert_eq!(result.get_u64_ne(), 0x3333_3333_3333_3333);
    assert_eq!(result.get_u32_ne(), 0x4444_4444);
    assert_eq!(result.get_u32_ne(), 0x5555_5555);
    assert_eq!(result.get_u64_ne(), 0x6666_6666_6666_6666);
}

#[test]
fn vectored_write_max_len_overflow() {
    let mut buf = BytesBuf::new();

    let memory = TestMemory::with_block_size(nz!(8));

    // Capacity: 0 -> 24 (3x8)
    buf.reserve(THREE_U64_SIZE, &memory);

    assert_eq!(buf.capacity(), 24);
    assert_eq!(buf.remaining_capacity(), 24);

    // We ask for 25 bytes of capacity but there are only 24 available. Oops!
    assert_panic!(buf.begin_vectored_write(Some(25)));
}

#[test]
fn vectored_write_overcommit() {
    let mut buf = BytesBuf::new();

    assert_eq!(buf.capacity(), 0);
    assert_eq!(buf.remaining_capacity(), 0);

    let memory = TestMemory::with_block_size(nz!(8));

    // Capacity: 0 -> 16 (2x8)
    buf.reserve(TWO_U64_SIZE, &memory);

    assert_eq!(buf.capacity(), 16);
    assert_eq!(buf.remaining_capacity(), 16);

    let vectored_write = buf.begin_vectored_write(None);

    assert_panic!(
        // SAFETY: Intentionally lying here to trigger a panic.
        unsafe {
            vectored_write.commit(17);
        }
    );
}

#[test]
fn vectored_write_abort() {
    let mut buf = BytesBuf::new();

    assert_eq!(buf.capacity(), 0);
    assert_eq!(buf.remaining_capacity(), 0);

    let memory = TestMemory::with_block_size(nz!(8));

    // Capacity: 0 -> 8 (1x8)
    buf.reserve(U64_SIZE, &memory);

    assert_eq!(buf.capacity(), 8);
    assert_eq!(buf.remaining_capacity(), 8);

    let mut vectored_write = buf.begin_vectored_write(None);

    let mut slices: Vec<_> = vectored_write.slices_mut().map(|(s, _)| s).collect();
    assert_eq!(slices.len(), 1);
    assert_eq!(slices[0].len(), 8);

    write_copy_of_slice(slices[0], &0x3333_3333_3333_3333_u64.to_ne_bytes());

    // Actually never mind - we drop it here.
    #[expect(clippy::drop_non_drop, reason = "Just being explicit for illustration")]
    drop(vectored_write);

    assert_eq!(buf.len(), 0);
    assert_eq!(buf.remaining_capacity(), 8);
    assert_eq!(buf.capacity(), 8);
}

#[test]
#[cfg(feature = "std")]
fn from_view() {
    let memory = GlobalPool::new();

    let view1 = BytesView::copied_from_slice(b"bla bla bla", &memory);

    let mut buf: BytesBuf = view1.clone().into();

    let view2 = buf.consume_all();

    assert_eq!(view1, view2);
}

#[test]
fn peek_empty_builder() {
    let buf = BytesBuf::new();
    let peeked = buf.peek();

    assert!(peeked.is_empty());
    assert_eq!(peeked.len(), 0);
}

#[test]
fn peek_with_frozen_spans_only() {
    let memory = TestMemory::with_block_size(nz!(10));
    let mut buf = BytesBuf::new();

    buf.reserve(20, &memory);
    buf.put_u64_ne(0x1111_1111_1111_1111);
    buf.put_u64_ne(0x2222_2222_2222_2222);
    // Both blocks are now frozen (filled completely)
    assert_eq!(buf.len(), 16);

    let mut peeked = buf.peek();

    assert_eq!(peeked.len(), 16);
    assert_eq!(peeked.get_u64_ne(), 0x1111_1111_1111_1111);
    assert_eq!(peeked.get_u64_ne(), 0x2222_2222_2222_2222);

    // Original builder still has the data
    assert_eq!(buf.len(), 16);
}

#[test]
fn peek_with_partially_filled_span_builder() {
    let memory = TestMemory::with_block_size(nz!(10));
    let mut buf = BytesBuf::new();

    buf.reserve(10, &memory);
    buf.put_u64_ne(0x3333_3333_3333_3333);
    buf.put_u16_ne(0x4444);
    // We have 10 bytes filled in a 10-byte block
    assert_eq!(buf.len(), 10);

    let mut peeked = buf.peek();

    assert_eq!(peeked.len(), 10);
    assert_eq!(peeked.get_u64_ne(), 0x3333_3333_3333_3333);
    assert_eq!(peeked.get_u16_ne(), 0x4444);

    // Original builder still has the data
    assert_eq!(buf.len(), 10);
}

#[test]
fn peek_preserves_capacity_of_partial_span_builder() {
    let memory = TestMemory::with_block_size(nz!(20));
    let mut buf = BytesBuf::new();

    buf.reserve(20, &memory);
    buf.put_u64_ne(0x5555_5555_5555_5555);

    // We have 8 bytes filled and 12 bytes remaining capacity
    assert_eq!(buf.len(), 8);
    assert_eq!(buf.remaining_capacity(), 12);

    let mut peeked = buf.peek();

    assert_eq!(peeked.len(), 8);
    assert_eq!(peeked.get_u64_ne(), 0x5555_5555_5555_5555);

    // CRITICAL TEST: Capacity should be preserved
    assert_eq!(buf.len(), 8);
    assert_eq!(buf.remaining_capacity(), 12);

    // We should still be able to write more data
    buf.put_u32_ne(0x6666_6666);
    assert_eq!(buf.len(), 12);
    assert_eq!(buf.remaining_capacity(), 8);

    // And we can peek again to see the updated data
    let mut peeked2 = buf.peek();
    assert_eq!(peeked2.len(), 12);
    assert_eq!(peeked2.get_u64_ne(), 0x5555_5555_5555_5555);
    assert_eq!(peeked2.get_u32_ne(), 0x6666_6666);
}

#[test]
fn peek_with_mixed_frozen_and_unfrozen() {
    let memory = TestMemory::with_block_size(nz!(10));
    let mut buf = BytesBuf::new();

    buf.reserve(30, &memory);

    // Fill first block completely (10 bytes) - will be frozen
    buf.put_u64_ne(0x1111_1111_1111_1111);
    buf.put_u16_ne(0x2222);

    // Fill second block completely (10 bytes) - will be frozen
    buf.put_u64_ne(0x3333_3333_3333_3333);
    buf.put_u16_ne(0x4444);

    // Partially fill third block (only 4 bytes) - will remain unfrozen
    buf.put_u32_ne(0x5555_5555);

    assert_eq!(buf.len(), 24);
    assert_eq!(buf.remaining_capacity(), 6);

    let mut peeked = buf.peek();

    assert_eq!(peeked.len(), 24);
    assert_eq!(peeked.get_u64_ne(), 0x1111_1111_1111_1111);
    assert_eq!(peeked.get_u16_ne(), 0x2222);
    assert_eq!(peeked.get_u64_ne(), 0x3333_3333_3333_3333);
    assert_eq!(peeked.get_u16_ne(), 0x4444);
    assert_eq!(peeked.get_u32_ne(), 0x5555_5555);
    // Original builder still has all the data and capacity
    assert_eq!(buf.len(), 24);
    assert_eq!(buf.remaining_capacity(), 6);
}

#[test]
fn peek_then_consume() {
    let memory = TestMemory::with_block_size(nz!(20));
    let mut buf = BytesBuf::new();

    buf.reserve(20, &memory);
    buf.put_u64_ne(0x7777_7777_7777_7777);
    buf.put_u32_ne(0x8888_8888);
    assert_eq!(buf.len(), 12);

    // Peek at the data
    let mut peeked = buf.peek();
    assert_eq!(peeked.len(), 12);
    assert_eq!(peeked.get_u64_ne(), 0x7777_7777_7777_7777);

    // Original builder still has the data
    assert_eq!(buf.len(), 12);

    // Now consume some of it
    let mut consumed = buf.consume(8);
    assert_eq!(consumed.get_u64_ne(), 0x7777_7777_7777_7777);

    // Builder should have less data now
    assert_eq!(buf.len(), 4);

    // Peek again should show the remaining data
    let mut peeked2 = buf.peek();
    assert_eq!(peeked2.len(), 4);
    assert_eq!(peeked2.get_u32_ne(), 0x8888_8888);
}

#[test]
fn peek_multiple_times() {
    let memory = TestMemory::with_block_size(nz!(20));
    let mut buf = BytesBuf::new();

    buf.reserve(20, &memory);
    buf.put_u64_ne(0xAAAA_AAAA_AAAA_AAAA);

    // Peek multiple times - each should work independently
    let mut peeked1 = buf.peek();
    let mut peeked2 = buf.peek();

    assert_eq!(peeked1.get_u64_ne(), 0xAAAA_AAAA_AAAA_AAAA);
    assert_eq!(peeked2.get_u64_ne(), 0xAAAA_AAAA_AAAA_AAAA);

    // Original builder still intact
    assert_eq!(buf.len(), 8);
}

#[test]
fn first_unfilled_slice_meta_no_capacity() {
    let buf = BytesBuf::new();
    assert!(buf.first_unfilled_slice_meta().is_none());
}

#[test]
fn first_unfilled_slice_meta_no_meta() {
    let memory = TestMemory::with_block_size(nz!(64));
    let buf = memory.reserve(64);
    assert!(buf.first_unfilled_slice_meta().is_none());
}

#[test]
fn first_unfilled_slice_meta_with_meta() {
    #[derive(Debug)]
    struct CustomMeta;

    impl BlockMeta for CustomMeta {}

    // SAFETY: We are not allowed to drop this until all BlockRef are gone. This is fine
    // because it is dropped at the end of the function, after all BlockRef instances.
    let block = unsafe { TestMemoryBlock::new(nz!(100), Some(Box::new(CustomMeta))) };
    let block = pin!(block);

    // SAFETY: We guarantee exclusive access to the memory capacity.
    let memory_block = unsafe { block.as_ref().to_block() };

    let buf = BytesBuf::from_blocks([memory_block]);
    assert_eq!(block.ref_count(), 1);
    let meta = buf.first_unfilled_slice_meta().expect("should have metadata");
    assert!(meta.is::<CustomMeta>());
    assert!(!meta.is::<u8>());

    drop(buf);
    assert_eq!(block.ref_count(), 0);
}

#[test]
fn split_off_remaining_basic() {
    let memory = TestMemory::with_block_size(nz!(100));
    let mut buf = memory.reserve(100);

    buf.put_u64_ne(1234);

    let remaining_before_split = buf.remaining_capacity();

    let split = buf.split_off_remaining(20);

    // Original keeps its data and lost the split capacity.
    assert_eq!(buf.len(), U64_SIZE);
    assert_eq!(buf.remaining_capacity(), remaining_before_split - 20);

    // Split-off buffer has the requested capacity and no data.
    assert!(split.remaining_capacity() >= 20);
    assert!(split.is_empty());
}

#[test]
fn split_off_remaining_write_to_split() {
    let memory = TestMemory::with_block_size(nz!(100));
    let mut buf = memory.reserve(100);

    buf.put_u64_ne(1111);

    let mut split = buf.split_off_remaining(40);

    // Write into the split-off buffer.
    split.put_u64_ne(2222);
    split.put_u64_ne(3333);

    assert_eq!(split.len(), TWO_U64_SIZE);

    let mut split_view = split.consume_all();
    assert_eq!(split_view.get_u64_ne(), 2222);
    assert_eq!(split_view.get_u64_ne(), 3333);

    // Original buffer data is unaffected.
    let mut original_view = buf.consume_all();
    assert_eq!(original_view.get_u64_ne(), 1111);
}

#[test]
fn split_off_remaining_all_capacity() {
    let memory = TestMemory::with_block_size(nz!(100));
    let mut buf = memory.reserve(100);
    let full_capacity = buf.remaining_capacity();

    let split = buf.split_off_remaining(full_capacity);

    assert_eq!(buf.remaining_capacity(), 0);
    assert_eq!(split.remaining_capacity(), full_capacity);
}

#[test]
fn split_off_remaining_zero() {
    let memory = TestMemory::with_block_size(nz!(100));
    let mut buf = memory.reserve(100);
    let original_capacity = buf.remaining_capacity();

    let split = buf.split_off_remaining(0);

    assert_eq!(buf.remaining_capacity(), original_capacity);
    assert!(split.is_empty());
    assert_eq!(split.remaining_capacity(), 0);
}

#[test]
fn split_off_remaining_multi_block() {
    let memory = TestMemory::with_block_size(nz!(10));

    // Reserve enough to trigger multiple blocks.
    let mut buf = memory.reserve(30);

    buf.put_u64_ne(1234);

    let original_remaining = buf.remaining_capacity();

    // Split off more than one block's capacity to exercise draining + partial split.
    let mut split = buf.split_off_remaining(15);

    assert_eq!(buf.remaining_capacity(), original_remaining - 15);
    assert_eq!(buf.len(), U64_SIZE);

    // The split-off buffer capacity is usable.
    split.put_byte_repeated(0xAA, 15);
    assert_eq!(split.len(), 15);

    // Original data is unaffected.
    let mut original_view = buf.consume(U64_SIZE);
    assert_eq!(original_view.get_u64_ne(), 1234);
}

#[test]
fn split_off_remaining_empty_buffer_no_capacity() {
    let mut buf = BytesBuf::new();
    assert_eq!(buf.remaining_capacity(), 0);

    let split = buf.split_off_remaining(0);

    assert_eq!(buf.len(), 0);
    assert_eq!(buf.remaining_capacity(), 0);
    assert_eq!(split.len(), 0);
    assert_eq!(split.remaining_capacity(), 0);
}

#[test]
fn split_off_remaining_panics_on_overflow() {
    let memory = TestMemory::with_block_size(nz!(100));
    let mut buf = memory.reserve(100);
    let capacity = buf.remaining_capacity();

    assert_panic!(buf.split_off_remaining(capacity + 1));
}

#[test]
fn split_off_remaining_checked_returns_none() {
    let memory = TestMemory::with_block_size(nz!(100));
    let mut buf = memory.reserve(100);
    let capacity = buf.remaining_capacity();

    assert!(buf.split_off_remaining_checked(capacity + 1).is_none());

    // Buffer is unmodified.
    assert_eq!(buf.remaining_capacity(), capacity);
}
