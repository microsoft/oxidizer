// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Tests buffer ranges, writes, reads, and shared memory lifetimes.

#![cfg(all(test, feature = "test-util"))]

#[cfg(not(miri))]
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::hash::{Hash, Hasher};
use std::io::{Read, Write};
use std::mem::MaybeUninit;
use std::num::NonZero;
use std::ops::Bound::{Excluded, Included, Unbounded};
use std::sync::atomic::{AtomicUsize, Ordering};

use bytesbuf::BytesView;
use bytesbuf::mem::testing::{FixedBlockMemory, TransparentMemory};
use bytesbuf::mem::{CallbackMemory, GlobalPool};

#[cfg(not(miri))]
struct GammaSurvivorAllocator;
static GAMMA_SURVIVOR_POINTER: AtomicUsize = AtomicUsize::new(0);
static GAMMA_SURVIVOR_FREES: AtomicUsize = AtomicUsize::new(0);

thread_local! {
    static GAMMA_SURVIVOR_CAPTURE: Cell<bool> = const { Cell::new(false) };
    static GAMMA_SURVIVOR_ALLOCATION_LOG: Cell<([usize; 8], usize, u8)> = const { Cell::new(([0; 8], 0, 0)) };
}

// SAFETY: All allocation operations delegate unchanged to System.
#[cfg(not(miri))]
unsafe impl GlobalAlloc for GammaSurvivorAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: The caller supplies a valid allocation layout.
        let pointer = unsafe { System.alloc(layout) };
        if GAMMA_SURVIVOR_CAPTURE.try_with(Cell::get).unwrap_or(false) {
            let _ = GAMMA_SURVIVOR_ALLOCATION_LOG.try_with(|log| {
                let (mut addresses, count, freed) = log.get();
                if count < addresses.len() {
                    addresses[count] = pointer.addr();
                }
                log.set((addresses, count + 1, freed));
            });
        }
        pointer
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        let _ = GAMMA_SURVIVOR_ALLOCATION_LOG.try_with(|log| {
            let (addresses, count, mut freed) = log.get();
            for (index, &address) in addresses.iter().enumerate().take(count) {
                if address == ptr.addr() {
                    freed |= 1 << index;
                }
            }
            log.set((addresses, count, freed));
        });
        if GAMMA_SURVIVOR_POINTER
            .compare_exchange(ptr.addr(), 0, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
        {
            GAMMA_SURVIVOR_FREES.fetch_add(1, Ordering::Relaxed);
        }
        // SAFETY: The pointer and layout are forwarded to their original allocator.
        unsafe { System.dealloc(ptr, layout) };
    }
}

#[global_allocator]
#[cfg(not(miri))]
static GAMMA_SURVIVOR_ALLOCATOR: GammaSurvivorAllocator = GammaSurvivorAllocator;

fn gamma_survivor_segmented(data: &[u8], width: usize) -> BytesView {
    BytesView::from_views(data.chunks(width).map(|chunk| BytesView::from(chunk.to_vec())))
}

#[test]
fn gamma_survivor_range_bound_matrix() {
    let data: Vec<u8> = (0..23).collect();
    for width in [1, 2, 3, 4, 7, 23] {
        let view = gamma_survivor_segmented(&data, width);
        for start in 0..=data.len() {
            for end in start..=data.len() {
                let range = view.range_checked(start..end).unwrap();
                assert_eq!(range.len(), end - start);
                assert_eq!(range.to_vec(), data[start..end]);
                if start == end {
                    assert_eq!(range.slices().count(), 0);
                }
                assert_eq!(view.range_checked((Included(start), Excluded(end))).unwrap(), &data[start..end]);
                if start > 0 {
                    assert_eq!(view.range_checked((Excluded(start - 1), Excluded(end))).unwrap(), &data[start..end]);
                }
                if end > start {
                    let inclusive_end = end - 1;
                    assert_eq!(view.range_checked(start..=inclusive_end).unwrap(), &data[start..end]);
                }
            }
            assert_eq!(view.range_checked(start..).unwrap(), &data[start..]);
        }
        for bounds in [
            (Excluded(usize::MAX), Unbounded),
            (Included(0), Included(usize::MAX)),
            (Included(2), Excluded(1)),
            (Included(24), Unbounded),
            (Included(23), Excluded(24)),
        ] {
            assert!(view.range_checked(bounds).is_none());
        }
        assert_eq!(view.to_vec(), data);
    }
}

#[test]
#[cfg_attr(miri, ignore = "native tests cover allocation counts; buffer ranges retain direct Miri coverage")]
fn gamma_survivor_small_range_keeps_metadata_inline() {
    for length in [8, 80] {
        let data = vec![37; length];
        let view = gamma_survivor_segmented(&data, 1);
        GAMMA_SURVIVOR_ALLOCATION_LOG.with(|log| log.set(([0; 8], 0, 0)));
        GAMMA_SURVIVOR_CAPTURE.with(|capture| capture.set(true));
        let range = view.range(1..7);
        GAMMA_SURVIVOR_CAPTURE.with(|capture| capture.set(false));
        let (_, count, _) = GAMMA_SURVIVOR_ALLOCATION_LOG.with(Cell::get);
        assert_eq!(count, 0);
        assert_eq!(range.len(), 6);
        assert_eq!(range, [37; 6].as_slice());
        drop(range);
        GAMMA_SURVIVOR_ALLOCATION_LOG.with(|log| log.set(([0; 8], 0, 0)));
    }
}

#[test]
fn gamma_survivor_copy_numeric_equality_and_consumption() {
    let data: Vec<u8> = (0..40).collect();
    for width in [1, 2, 4, 8, 13, 40] {
        for count in 0..=40 {
            let original = gamma_survivor_segmented(&data, width);
            let mut view = original.clone();
            let mut destination = vec![255; count];
            view.copy_to_slice(&mut destination);
            assert_eq!(destination, data[..count]);
            assert_eq!(view, &data[count..]);
            assert_ne!(original, &data[..39]);
            let mut changed = data.clone();
            changed[39] = 255;
            assert_ne!(original, changed.as_slice());

            let mut uninit = vec![MaybeUninit::uninit(); count];
            let mut view = original.clone();
            view.copy_to_uninit_slice(&mut uninit);
            // SAFETY: copy_to_uninit_slice initializes every element of the destination.
            let copied: Vec<_> = uninit.iter().map(|byte| unsafe { byte.assume_init() }).collect();
            assert_eq!(copied, data[..count]);
            assert_eq!(view, &data[count..]);
        }
        let mut view = gamma_survivor_segmented(&data, width);
        assert_eq!(view.get_u16_be(), 0x0001);
        assert_eq!(view.get_u32_le(), 0x0504_0302);
        assert_eq!(view.get_u64_be(), 0x0607_0809_0a0b_0c0d);
        assert_eq!(view, &data[14..]);
        let mut gathered = Vec::new();
        view.consume_all_slices(|slice| gathered.extend_from_slice(slice));
        assert_eq!(gathered, data[14..]);
        assert!(view.is_empty());
        assert!(view.first_slice().is_empty());
        let mut destination = [255];
        assert_eq!(view.read(&mut destination).unwrap(), 0);
        assert_eq!(destination, [255]);
    }
}

#[derive(Default)]
struct GammaSurvivorHash(Vec<Vec<u8>>);

impl Hasher for GammaSurvivorHash {
    fn finish(&self) -> u64 {
        0
    }

    fn write(&mut self, bytes: &[u8]) {
        self.0.push(bytes.to_vec());
    }
}

#[test]
fn gamma_survivor_hash_has_exact_length_and_bytes_across_boundaries() {
    for length in [0_usize, 1, 63, 64, 65, 127, 128, 129, 257] {
        let data: Vec<u8> = (0..length).map(|index| u8::try_from(index % 251).unwrap()).collect();
        let mut expected = GammaSurvivorHash::default();
        BytesView::from(data.clone()).hash(&mut expected);
        assert_eq!(expected.0[0], length.to_ne_bytes());
        assert_eq!(expected.0[1..].concat(), data);
        for width in [1, 3, 17, 64, 65] {
            let mut actual = GammaSurvivorHash::default();
            gamma_survivor_segmented(&data, width).hash(&mut actual);
            assert_eq!(actual.0, expected.0);
        }
    }
}

#[test]
fn gamma_survivor_buffer_split_consume_and_peek_matrix() {
    let memory = FixedBlockMemory::new(NonZero::new(5).unwrap());
    for written in 0..=20 {
        for split in 0..=20 - written {
            let mut buffer = memory.reserve(20);
            let mut data: Vec<u8> = (0..written).map(|index| u8::try_from(index).unwrap()).collect();
            buffer.put_slice(data.as_slice());
            assert_eq!(buffer.peek(), data.as_slice());
            assert_eq!(buffer.remaining_capacity(), 20 - written);
            assert!(buffer.split_off_remaining_checked(21 - written).is_none());
            let mut spare = buffer.split_off_remaining_checked(split).unwrap();
            assert_eq!(spare.capacity(), split);
            assert_eq!(buffer.remaining_capacity(), 20 - written - split);
            spare.put_byte_repeated(99, split);
            assert_eq!(spare.consume_all(), vec![99; split].as_slice());
            assert_eq!(buffer.peek(), data.as_slice());
            // Freeze all original data before partially consuming a builder with no spare capacity.
            buffer.put_bytes(BytesView::from(vec![211]));
            data.push(211);
            assert!(buffer.consume_checked(data.len() + 1).is_none());
            let prefix = written / 2;
            assert_eq!(buffer.consume(prefix), &data[..prefix]);
            assert_eq!(buffer.peek(), &data[prefix..]);
            assert_eq!(buffer.consume_all(), &data[prefix..]);
            assert!(buffer.peek().is_empty());
        }
    }
}

#[test]
fn gamma_survivor_append_preserves_order_and_spare_capacity() {
    let memory = TransparentMemory::new();
    let mut buffer = memory.reserve(9);
    buffer.put_slice(*b"ab");
    buffer.put_bytes(BytesView::from(Vec::new()));
    assert_eq!((buffer.len(), buffer.remaining_capacity()), (2, 7));
    buffer.put_bytes(gamma_survivor_segmented(b"cdef", 2));
    buffer.put_slice(*b"gh");
    assert_eq!((buffer.len(), buffer.remaining_capacity()), (8, 5));
    assert_eq!(buffer.peek(), b"abcdefgh");
    assert_eq!(buffer.consume(3), b"abc");
    assert_eq!(buffer.consume_all(), b"defgh");
    buffer.put_slice(*b"ijklm");
    assert_eq!(buffer.consume_all(), b"ijklm");
}

#[test]
fn gamma_survivor_vectored_write_offsets_and_zero_commit() {
    let memory = FixedBlockMemory::new(NonZero::new(4).unwrap());
    for limit in 0..=12 {
        let mut buffer = memory.reserve(12);
        let transaction = buffer.begin_vectored_write_checked(Some(13));
        assert!(transaction.is_none());
        let mut transaction = buffer.begin_vectored_write(Some(limit));
        let mut count = 0;
        let mut lengths = Vec::new();
        for (slice, _) in transaction.slices_mut() {
            lengths.push(slice.len());
            for byte in slice {
                byte.write(u8::try_from(count).unwrap());
                count += 1;
            }
        }
        assert_eq!(count, limit);
        assert_eq!(
            lengths,
            if limit == 0 {
                vec![0]
            } else {
                (0..limit).step_by(4).map(|offset| (limit - offset).min(4)).collect::<Vec<_>>()
            }
        );
        // SAFETY: Exactly limit bytes were initialized in the transaction's logical prefix.
        unsafe { transaction.commit(limit) };
        assert_eq!(buffer.len(), limit);
        assert_eq!(buffer.remaining_capacity(), 12 - limit);
        assert_eq!(
            buffer.consume_all().to_vec(),
            (0..limit).map(|index| u8::try_from(index).unwrap()).collect::<Vec<_>>()
        );
    }
    let mut buffer = memory.reserve(8);
    // SAFETY: Advancing zero bytes requires no initialization.
    unsafe { buffer.advance(0) };
    assert_eq!((buffer.len(), buffer.remaining_capacity()), (0, 8));
}

#[test]
fn gamma_survivor_writer_reservation_boundaries_and_counts() {
    let memory = TransparentMemory::new();
    for (initial, payload, expected_capacity) in [(0, 0, 0), (0, 1, 4096), (0, 4096, 4096), (0, 4097, 65536), (4, 4, 4), (4, 5, 65536)] {
        let mut writer = memory.reserve(initial).into_writer(&memory);
        let data = vec![73; payload];
        assert_eq!(writer.write(&data).unwrap(), payload);
        writer.flush().unwrap();
        let mut buffer = writer.into_inner();
        assert_eq!(buffer.capacity(), expected_capacity);
        assert_eq!(buffer.consume_all(), data.as_slice());
    }
}

#[test]
#[cfg_attr(
    miri,
    ignore = "native tests cover allocator callbacks; buffer ownership retains direct Miri coverage"
)]
fn gamma_survivor_vec_backing_freed_only_after_last_guard() {
    let data = vec![13, 17, 19, 23, 29];
    GAMMA_SURVIVOR_FREES.store(0, Ordering::Relaxed);
    GAMMA_SURVIVOR_POINTER.store(data.as_ptr().addr(), Ordering::Relaxed);
    let view = BytesView::from(data);
    let clone = view.clone();
    let range = view.range(1..4);
    let guard = range.extend_lifetime();
    drop(view);
    assert_eq!(GAMMA_SURVIVOR_FREES.load(Ordering::Relaxed), 0);
    assert_eq!(clone, [13, 17, 19, 23, 29].as_slice());
    drop(clone);
    drop(range);
    assert_eq!(GAMMA_SURVIVOR_FREES.load(Ordering::Relaxed), 0);
    drop(guard);
    assert_eq!(GAMMA_SURVIVOR_FREES.load(Ordering::Relaxed), 1);
    assert_eq!(GAMMA_SURVIVOR_POINTER.load(Ordering::Relaxed), 0);

    GAMMA_SURVIVOR_ALLOCATION_LOG.with(|log| log.set(([0; 8], 0, 0)));
    GAMMA_SURVIVOR_CAPTURE.with(|capture| capture.set(true));
    let mut buffer = TransparentMemory::new().reserve(17);
    GAMMA_SURVIVOR_CAPTURE.with(|capture| capture.set(false));
    let (_, count, _) = GAMMA_SURVIVOR_ALLOCATION_LOG.with(Cell::get);
    assert!((2..=8).contains(&count));
    buffer.put_byte_repeated(43, 17);
    let view = buffer.consume_all();
    let clone = view.clone();
    let guard = clone.extend_lifetime();
    drop(buffer);
    drop(view);
    assert_eq!(clone.to_vec(), vec![43; 17]);
    drop(clone);
    let (_, _, before) = GAMMA_SURVIVOR_ALLOCATION_LOG.with(Cell::get);
    assert_ne!(usize::from(before), (1_usize << count) - 1);
    drop(guard);
    let (_, _, after) = GAMMA_SURVIVOR_ALLOCATION_LOG.with(Cell::get);
    assert_eq!(usize::from(after), (1_usize << count) - 1);
    GAMMA_SURVIVOR_ALLOCATION_LOG.with(|log| log.set(([0; 8], 0, 0)));
}

#[test]
fn gamma_survivor_pool_reservation_and_live_clone_reuse() {
    let memory = GlobalPool::new();
    for capacity in [
        0_usize, 1, 1023, 1024, 1025, 4095, 4096, 4097, 16_383, 16_384, 16_385, 65_535, 65_536, 65_537, 262_145,
    ] {
        let mut buffer = memory.reserve(capacity);
        let block_size = match capacity {
            0..=1024 => 1024,
            1025..=4096 => 4096,
            4097..=16384 => 16384,
            _ => 65536,
        };
        assert_eq!(buffer.capacity(), capacity.div_ceil(block_size) * block_size);
        assert!(buffer.is_empty());
        buffer.put_byte_repeated(31, capacity);
        let retained = buffer.consume_all();
        let clone = retained.clone();
        drop(retained);
        let mut other = memory.reserve(capacity);
        other.put_byte_repeated(97, capacity);
        assert_eq!(clone.to_vec(), vec![31; capacity]);
        assert_eq!(other.consume_all().to_vec(), vec![97; capacity]);
    }
    let callback = CallbackMemory::new(TransparentMemory::new(), TransparentMemory::reserve);
    let rendered = format!("{callback:?}");
    assert!(rendered.starts_with("CallbackMemory { reserve_fn: "));
    assert!(rendered.ends_with(", .. }"));
}

#[cfg(feature = "bytes-compat")]
#[test]
fn gamma_survivor_bytes_owner_lifetime_and_segmented_roundtrip() {
    use std::sync::Arc;

    struct GammaSurvivorOwner {
        bytes: [u8; 5],
        drops: Arc<AtomicUsize>,
    }
    impl AsRef<[u8]> for GammaSurvivorOwner {
        fn as_ref(&self) -> &[u8] {
            &self.bytes
        }
    }
    impl Drop for GammaSurvivorOwner {
        fn drop(&mut self) {
            self.drops.fetch_add(1, Ordering::Relaxed);
        }
    }
    let drops = Arc::new(AtomicUsize::new(0));
    let bytes = bytes::Bytes::from_owner(GammaSurvivorOwner {
        bytes: [2, 3, 5, 7, 11],
        drops: Arc::clone(&drops),
    });
    let view = BytesView::from(bytes);
    let clone = view.clone();
    let guard = clone.extend_lifetime();
    drop(view);
    assert_eq!(drops.load(Ordering::Relaxed), 0);
    assert_eq!(clone, [2, 3, 5, 7, 11].as_slice());
    drop(clone);
    assert_eq!(drops.load(Ordering::Relaxed), 0);
    drop(guard);
    assert_eq!(drops.load(Ordering::Relaxed), 1);
    for width in [1, 2, 5] {
        let view = gamma_survivor_segmented(b"abcde", width);
        let bytes: bytes::Bytes = view.into();
        assert_eq!(bytes.as_ref(), b"abcde");
    }
    let empty: bytes::Bytes = BytesView::new().into();
    assert!(empty.is_empty());
    let mut view = gamma_survivor_segmented(b"abcde", 2);
    let mut empty = [];
    bytes::Buf::copy_to_slice(&mut view, &mut empty);
    assert_eq!(bytes::Buf::remaining(&view), 5);
    let mut destination = [0; 3];
    bytes::Buf::copy_to_slice(&mut view, &mut destination);
    assert_eq!(&destination, b"abc");
    assert_eq!(view, b"de");
}
