// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! End-to-end regression guards for the chunk allocation *footprint*:
//! the number of bytes actually requested from the underlying allocator
//! must match the chunk's size class, not be inflated to `CHUNK_ALIGN`
//! (64 KiB). This is the gap that previously let every shared chunk be
//! silently allocated at 64 KiB while the byte budget / stats tracked the
//! much smaller unpadded size.

#![cfg(feature = "std")]
#![allow(clippy::unwrap_used, reason = "test code")]
#![allow(clippy::std_instead_of_core, reason = "test code uses std")]
#![allow(clippy::collection_is_never_read, reason = "tests retain handles to keep chunks alive")]

use std::alloc::Layout;
use std::ptr::NonNull;
use std::sync::{Arc, Mutex};

use allocator_api2::alloc::{AllocError, Allocator, Global};
use multitude::Arena;

/// `CHUNK_ALIGN` / `MAX_CHUNK_BYTES`: the design-fixed shared-chunk base
/// alignment and maximum cacheable chunk size. Mirrored here because the
/// constant is crate-internal.
const CHUNK_ALIGN: usize = 65_536;

/// Shared log of `(size, align)` allocation requests.
type RequestLog = Arc<Mutex<Vec<(usize, usize)>>>;

/// Records the `(size, align)` of every allocation request so a test can
/// assert what was actually asked of the underlying allocator. The log is
/// an `Arc` (not a leaked `'static`) so nothing leaks under Miri.
#[derive(Clone)]
struct Recorder {
    requests: RequestLog,
}

// SAFETY: forwards faithfully to `Global`; only observes the layout.
unsafe impl Allocator for Recorder {
    fn allocate(&self, layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
        self.requests.lock().unwrap().push((layout.size(), layout.align()));
        Global.allocate(layout)
    }

    unsafe fn deallocate(&self, ptr: NonNull<u8>, layout: Layout) {
        // SAFETY: forwarded per the `Allocator` contract.
        unsafe { Global.deallocate(ptr, layout) };
    }
}

fn recorder() -> (Recorder, RequestLog) {
    let requests = Arc::new(Mutex::new(Vec::new()));
    (
        Recorder {
            requests: Arc::clone(&requests),
        },
        requests,
    )
}

/// Sizes of the allocations that landed on a `CHUNK_ALIGN` boundary — i.e.
/// the shared chunks.
fn shared_chunk_sizes(log: &RequestLog) -> Vec<usize> {
    log.lock()
        .unwrap()
        .iter()
        .filter(|(_, align)| *align >= CHUNK_ALIGN)
        .map(|(size, _)| *size)
        .collect()
}

/// A single small `Arc<u32>` needs only a class-0 shared chunk (512 B).
/// The allocator must therefore be asked for far less than `CHUNK_ALIGN`
/// bytes — before the fix it was asked for exactly 64 KiB.
#[test]
fn small_shared_chunk_is_not_inflated_to_chunk_align() {
    let (rec, log) = recorder();
    {
        let arena = Arena::new_in(rec);
        let a = arena.alloc_arc(7_u32);
        assert_eq!(*a, 7);

        let shared = shared_chunk_sizes(&log);
        assert!(
            !shared.is_empty(),
            "expected at least one shared-chunk allocation, got {:?}",
            log.lock().unwrap()
        );
        for size in &shared {
            assert!(
                *size < CHUNK_ALIGN,
                "a shared chunk for a single small Arc was allocated at {size} B (>= {CHUNK_ALIGN}); the size was inflated to the base alignment"
            );
        }
    }
}

/// The byte-accounting gauge must match the real allocator footprint: with
/// the size inflated to 64 KiB but the budget tracking the unpadded size,
/// `total_bytes_allocated` severely under-reported. After the fix the real
/// shared-chunk bytes equal the tracked bytes.
#[cfg(feature = "stats")]
#[test]
fn total_bytes_allocated_matches_real_shared_footprint() {
    let (rec, log) = recorder();
    let arena = Arena::new_in(rec);
    let _a = arena.alloc_arc(123_u64);

    let real_shared_bytes: usize = shared_chunk_sizes(&log).iter().sum();
    assert!(
        real_shared_bytes > 0,
        "expected a shared-chunk allocation, got {:?}",
        log.lock().unwrap()
    );

    let tracked = arena.stats().total_bytes_allocated;
    assert_eq!(
        tracked, real_shared_bytes as u64,
        "total_bytes_allocated ({tracked}) must equal the real shared-chunk bytes requested from the allocator ({real_shared_bytes})"
    );
}

/// Allocating many small Arcs creates several shared chunks as the bump
/// cursor exhausts each one. Their sizes must be power-of-two class sizes
/// capped at `CHUNK_ALIGN`, with at least one strictly below it — before
/// the fix *every* shared chunk was exactly 64 KiB.
#[test]
fn shared_chunk_sizes_track_size_classes() {
    let (rec, log) = recorder();
    let arena = Arena::new_in(rec);

    let mut handles = Vec::new();
    for i in 0..2_000_u32 {
        handles.push(arena.alloc_arc(i));
    }

    let shared = shared_chunk_sizes(&log);
    assert!(shared.len() >= 2, "expected multiple shared chunks, got sizes {shared:?}");
    assert!(
        shared.iter().any(|&s| s < CHUNK_ALIGN),
        "every shared chunk was allocated at {CHUNK_ALIGN} B: sizes {shared:?}"
    );
    for &s in &shared {
        assert!(s <= CHUNK_ALIGN, "chunk size {s} exceeds max chunk bytes");
        assert!(s.is_power_of_two(), "chunk size {s} is not a power-of-two class size");
    }
}
