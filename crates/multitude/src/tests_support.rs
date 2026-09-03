// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Shared test helpers used by per-module unit tests in `src/`.
//!
//! Only compiled under `cfg(test)`.

#![expect(dead_code, reason = "shared test scaffolding; not every helper is used by every test module")]
#![cfg_attr(coverage_nightly, coverage(off))]

use core::alloc::Layout;
use core::ptr::NonNull;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use allocator_api2::alloc::{AllocError, Allocator, Global};

use crate::Arena;

/// Chunk-alignment cap installed by [`capped_arena`].
///
/// The real cap is [`CHUNK_ALIGN`](crate::internal::constants::CHUNK_ALIGN),
/// 64 KiB, and the smart-pointer cap is half of that. No test can name a
/// type aligned that far — codegen backends differ in the maximum type
/// alignment they accept, and 32 KiB is above the lowest of them. Lowering
/// the caps puts both boundaries within reach of an alignment every backend
/// compiles, so the rejection guards can be driven everywhere.
pub(crate) const TEST_CHUNK_ALIGN: usize = 8192;

/// Smart-pointer alignment cap under [`TEST_CHUNK_ALIGN`]. Half the chunk
/// cap, as in production.
pub(crate) const TEST_SMART_PTR_ALIGN: usize = TEST_CHUNK_ALIGN / 2;

/// Arena whose alignment caps are low enough for a test to reach them.
///
/// Do not use for `Vec` / `String` growth or freeze tests. `buffer_freezable`
/// still compares against the real cap, so this arena classifies a
/// `TEST_SMART_PTR_ALIGN`-aligned element as freezable while the allocation
/// guards reject it — the two disagree only here, never in production.
pub(crate) fn capped_arena() -> Arena {
    let arena = Arena::new();
    arena.set_align_cap(TEST_CHUNK_ALIGN);
    arena
}

/// Aligned exactly at the smart-pointer cap: rejected by every
/// smart-pointer entry point, accepted by the simple-reference slice paths.
#[derive(Clone, Copy, Debug)]
#[cfg_attr(feature = "bytemuck", derive(bytemuck::Zeroable))]
#[cfg_attr(feature = "zerocopy", derive(zerocopy::FromZeros))]
#[repr(C, align(4096))]
pub(crate) struct SmartPtrOverAligned(pub(crate) u8);

/// [`SmartPtrOverAligned`] with a destructor, for the paths that branch on
/// `needs_drop::<T>()`.
#[derive(Debug)]
#[repr(C, align(4096))]
pub(crate) struct SmartPtrOverAlignedDrop(pub(crate) u8);

#[expect(clippy::empty_drop, reason = "the impl exists to make needs_drop::<T>() true")]
impl Drop for SmartPtrOverAlignedDrop {
    fn drop(&mut self) {}
}

/// Aligned exactly at the chunk cap: no chunk can satisfy it, so even the
/// simple-reference paths reject it.
#[derive(Clone, Copy, Debug)]
#[cfg_attr(feature = "bytemuck", derive(bytemuck::Zeroable))]
#[cfg_attr(feature = "zerocopy", derive(zerocopy::FromZeros))]
#[repr(C, align(8192))]
pub(crate) struct ChunkOverAligned(pub(crate) u8);

// `repr(align)` takes a literal, so the helper types cannot name the cap
// constants directly. Keep them in step.
const _: () = {
    assert!(align_of::<SmartPtrOverAligned>() == TEST_SMART_PTR_ALIGN);
    assert!(align_of::<SmartPtrOverAlignedDrop>() == TEST_SMART_PTR_ALIGN);
    assert!(align_of::<ChunkOverAligned>() == TEST_CHUNK_ALIGN);
};

/// Send-and-Sync allocator that fails after a fixed number of successful
/// allocations.
#[derive(Clone)]
struct FailingAllocator {
    remaining: Arc<AtomicUsize>,
}

impl FailingAllocator {
    fn new(allow_n_allocs: usize) -> Self {
        Self {
            remaining: Arc::new(AtomicUsize::new(allow_n_allocs)),
        }
    }
}

// SAFETY: forwards to Global; counters are atomic.
unsafe impl Allocator for FailingAllocator {
    fn allocate(&self, layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
        let mut cur = self.remaining.load(Ordering::Relaxed);
        loop {
            if cur == 0 {
                return Err(AllocError);
            }
            match self
                .remaining
                .compare_exchange_weak(cur, cur - 1, Ordering::Relaxed, Ordering::Relaxed)
            {
                Ok(_) => break,
                Err(actual) => cur = actual,
            }
        }
        Global.allocate(layout)
    }

    unsafe fn deallocate(&self, ptr: NonNull<u8>, layout: Layout) {
        // SAFETY: forwarded per Allocator contract.
        unsafe { Global.deallocate(ptr, layout) };
    }
}
