// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Pool ownership, exact allocation statistics, and reuse across chunks.

use std::alloc::Layout;
use std::ptr::NonNull;
use std::sync::{Arc as Shared, Mutex};

use allocator_api2::alloc::{AllocError, Allocator, Global};
use plurality::{Arc, MultiPool, Pool};

#[cfg(test)]
#[derive(Clone, Debug, Default)]
struct TrackedAllocator {
    allocated: Shared<Mutex<Vec<Layout>>>,
    freed: Shared<Mutex<Vec<Layout>>>,
}

// SAFETY: allocation and deallocation use Global with the unchanged pointer and layout.
#[cfg(test)]
unsafe impl Allocator for TrackedAllocator {
    fn allocate(&self, layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
        let result = Global.allocate(layout)?;
        self.allocated.lock().unwrap().push(layout);
        Ok(result)
    }

    unsafe fn deallocate(&self, ptr: NonNull<u8>, layout: Layout) {
        self.freed.lock().unwrap().push(layout);
        // SAFETY: the caller supplies a live allocation made with this same layout.
        unsafe { Global.deallocate(ptr, layout) };
    }
}

#[test]
fn gamma_survivor_slots_from_every_chunk_are_reusable_and_shared_handles_release_once() {
    let pool = Pool::<u32>::builder().chunk_size(2).max_chunks(3).build();
    let mut handles: Vec<_> = (0..6).map(|value| pool.alloc_arc(value)).collect();
    pool.try_alloc_arc(99).unwrap_err();
    let clone = Arc::clone(&handles[3]);
    let released = handles.remove(3);
    drop(released);
    assert_eq!(*clone, 3);
    pool.try_alloc_arc(99).unwrap_err();
    drop(clone);
    handles.push(pool.try_alloc_arc(17).unwrap());
    drop(handles);
    assert_eq!(pool.len(), 0);
    let reused: Vec<_> = (20..26).map(|value| pool.try_alloc_box(value).unwrap()).collect();
    assert_eq!(pool.chunks_allocated(), 3);
    assert_eq!(reused.iter().map(|value| **value).collect::<Vec<_>>(), (20..26).collect::<Vec<_>>());
    pool.try_alloc_box(99).unwrap_err();
}

#[test]
fn gamma_survivor_borrowed_uninit_transfers_slot_without_freeing_it() {
    let pool = Pool::<u32>::builder().chunk_size(1).max_chunks(1).build();
    let mut uninit = pool.alloc_uninit();
    uninit.write(17);
    // SAFETY: the full u32 was initialized immediately above.
    let initialized = unsafe { uninit.assume_init() };
    assert_eq!(*initialized, 17);
    pool.try_alloc(99).unwrap_err();
    drop(initialized);
    assert_eq!(*pool.try_alloc(23).unwrap(), 23);
}

#[test]
fn gamma_survivor_allocator_chunks_outlive_pool_until_last_handle_then_all_are_freed() {
    let allocator = TrackedAllocator::default();
    let pool = Pool::<u64>::builder().chunk_size(2).allocator(allocator.clone()).build();
    let first = pool.alloc_box(13);
    let second = pool.alloc_box(17);
    let third = pool.alloc_box(19);
    assert_eq!(allocator.allocated.lock().unwrap().len(), 2);
    drop(pool);
    assert_eq!(*allocator.freed.lock().unwrap(), Vec::<Layout>::new());
    drop((first, second));
    assert_eq!(*third, 19);
    assert_eq!(*allocator.freed.lock().unwrap(), Vec::<Layout>::new());
    drop(third);
    assert_eq!(*allocator.freed.lock().unwrap(), *allocator.allocated.lock().unwrap());

    let allocator = TrackedAllocator::default();
    let pool = MultiPool::builder().chunk_size(2).allocator(allocator.clone()).build();
    let first = pool.alloc_box(13_u32);
    let second = pool.alloc_box(17_u64);
    drop(pool);
    assert_eq!(*allocator.freed.lock().unwrap(), Vec::<Layout>::new());
    drop((first, second));
    assert_eq!(allocator.freed.lock().unwrap().len(), 2);
}

#[test]
fn gamma_survivor_byte_target_floor_and_layout_cap_are_exact() {
    assert_eq!(MultiPool::new().chunk_size_of::<u64>(), 256);
    assert_eq!(MultiPool::new().max_chunks_of::<u64>(), u32::MAX / 256);
    assert_eq!(MultiPool::builder().chunk_bytes(31).build().chunk_size_of::<u64>(), 1);
    assert_eq!(MultiPool::builder().chunk_bytes(32).build().chunk_size_of::<u64>(), 2);
    std::panic::catch_unwind(|| MultiPool::builder().chunk_size(0).build()).unwrap_err();
    let pool = MultiPool::builder().chunk_bytes(31).max_layouts(1).build();
    assert_eq!(pool.chunk_size_of::<u64>(), 1);
    let held = pool.alloc_box(13_u64);
    assert_eq!(pool.layouts(), 1);
    pool.try_alloc_box(17_u32).unwrap_err();
    assert_eq!(*held, 13);
    let output = format!("{pool:?}");
    for field in ["layouts:", "max_layouts:", "len:"] {
        assert!(output.contains(field), "{output}");
    }
    let pool = Pool::<u32>::builder().chunk_size(2).max_chunks(3).build();
    let output = format!("{pool:?}");
    for field in ["chunk_size: 2", "max_chunks: Some(3)", "chunks_allocated: 0", "len: 0"] {
        assert!(output.contains(field), "{output}");
    }
    let pool = MultiPool::builder().chunk_size(8).build();
    assert_eq!(pool.chunk_size_of::<u64>(), 8);
}

#[test]
fn gamma_survivor_directory_growth_preserves_all_chunks_and_distinct_layouts() {
    let pool = Pool::<u32>::builder().chunk_size(1).max_chunks(12).build();
    let held: Vec<_> = (0..12).map(|value| pool.alloc_box(value)).collect();
    assert_eq!(pool.chunks_allocated(), 12);
    assert_eq!(held.iter().map(|value| **value).collect::<Vec<_>>(), (0..12).collect::<Vec<_>>());
    drop(held);
    let reused: Vec<_> = (0..12).map(|value| pool.try_alloc_box(value).unwrap()).collect();
    assert_eq!(pool.chunks_allocated(), 12);
    assert_eq!(reused.len(), 12);
    assert_eq!(reused.iter().map(|value| **value).collect::<Vec<_>>(), (0..12).collect::<Vec<_>>());

    let allocator = TrackedAllocator::default();
    let multi = MultiPool::builder().chunk_size(1).allocator(allocator.clone()).build();
    let mut values: Vec<std::boxed::Box<dyn std::any::Any>> = Vec::new();
    macro_rules! hold {
        ($($size:literal),+) => { $(
            let held = multi.alloc_box([$size; $size]);
            assert_eq!(*held, [$size; $size]);
            values.push(std::boxed::Box::new(held));
        )+ };
    }
    hold!(1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12);
    assert_eq!(multi.layouts(), 12);
    assert_eq!(multi.len(), 12);
    drop(multi);
    assert_eq!(*allocator.freed.lock().unwrap(), Vec::<Layout>::new());
    drop(values);
    assert_eq!(allocator.freed.lock().unwrap().len(), 12);
}

#[cfg(feature = "stats")]
#[test]
fn gamma_survivor_multi_pool_byte_stats_equal_allocator_layouts_without_rounding() {
    let allocator = TrackedAllocator::default();
    let pool = MultiPool::builder().chunk_size(2).allocator(allocator.clone()).build();
    assert_eq!(pool.stats().total_bytes_allocated, 0);
    let held = [pool.alloc_box(13_u32), pool.alloc_box(17_u32), pool.alloc_box(19_u32)];
    let expected: u64 = allocator
        .allocated
        .lock()
        .unwrap()
        .iter()
        .map(|layout| u64::try_from(layout.size()).unwrap())
        .sum();
    assert_eq!(pool.stats().total_bytes_allocated, expected);
    assert_eq!(pool.stats().total_chunks_allocated, 2);
    drop(held);
    assert_eq!(pool.stats().total_bytes_allocated, expected);
}
