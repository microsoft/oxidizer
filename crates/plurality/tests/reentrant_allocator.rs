// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.
#![allow(clippy::allow_attributes, clippy::unwrap_used, reason = "test code")]

//! Proves that an allocator which allocates from the pool it serves is refused
//! at the boundary rather than corrupting pool state.
//!
//! Ref: docs/implementation/reentrancy.md.

use std::cell::Cell;
use std::ptr::{self, NonNull};

use allocator_api2::alloc::{AllocError as ApiAllocError, Allocator, Global};
use plurality::Pool;

// ─── the chunk-growth window ────────────────────────────────────────────

thread_local! {
    /// The pool a reentrant chunk allocation targets, armed for one entry.
    static GROWTH_TARGET: Cell<*const Pool<u32, ReentrantChunkAlloc>> = const { Cell::new(ptr::null()) };
    /// Whether the nested growth request was refused.
    static GROWTH_REFUSED: Cell<Option<bool>> = const { Cell::new(None) };
}

/// An allocator that allocates from the pool it serves, from inside the very
/// call that is growing that pool.
#[derive(Clone, Debug)]
struct ReentrantChunkAlloc;

// SAFETY: every request is forwarded to `Global`, which satisfies the contract;
// the reentrant probe allocates nothing of its own that outlives the call.
unsafe impl Allocator for ReentrantChunkAlloc {
    fn allocate(&self, layout: core::alloc::Layout) -> Result<NonNull<[u8]>, ApiAllocError> {
        let target = GROWTH_TARGET.replace(ptr::null());
        if !target.is_null() {
            // SAFETY: the test arms this with a pointer to a live pool and
            // disarms it above, so the borrow cannot outlive that pool.
            let pool = unsafe { &*target };
            GROWTH_REFUSED.set(Some(pool.try_alloc_box(7).is_err()));
        }
        Global.allocate(layout)
    }

    unsafe fn deallocate(&self, ptr: NonNull<u8>, layout: core::alloc::Layout) {
        // SAFETY: forwarded unchanged from a block `Global` produced.
        unsafe { Global.deallocate(ptr, layout) };
    }
}

#[test]
fn growth_refuses_a_reentrant_chunk_allocation() {
    let pool = Pool::<u32, _>::builder().allocator(ReentrantChunkAlloc).chunk_size(1).build();

    GROWTH_TARGET.set(&raw const pool);
    let first = pool.alloc_box(1);

    assert_eq!(
        GROWTH_REFUSED.get(),
        Some(true),
        "a nested allocation during growth must be refused"
    );
    assert_eq!(*first, 1);

    // The pool remains fully usable: the refusal rejected the nested request
    // without disturbing the growth that was in progress.
    let second = pool.alloc_box(2);
    assert_eq!(*second, 2);
    assert_eq!(pool.len(), 2);
}

// ─── the directory-reservation window ───────────────────────────────────

// This window is reached through the global allocator, and a global allocator
// forwarding to the system one is not meaningful under Miri's own allocator
// model. Ref: docs/implementation/verification.md, "Undefined-behaviour
// checking".
#[cfg(not(miri))]
use std::alloc::{GlobalAlloc, Layout as StdLayout, System};
#[cfg(not(miri))]
use std::panic::{AssertUnwindSafe, catch_unwind};

#[cfg(not(miri))]
use plurality::BlindPool;

#[cfg(not(miri))]
thread_local! {
    /// The router reentrant global allocations target while armed.
    static DIRECTORY_TARGET: Cell<*const BlindPool> = const { Cell::new(ptr::null()) };
    /// Set while a probe is running, so the probe's own allocations do not
    /// recurse into it.
    static PROBING: Cell<bool> = const { Cell::new(false) };
    /// How many nested routing requests were refused.
    static DIRECTORY_REFUSED: Cell<u32> = const { Cell::new(0) };
    /// How many were served, which happens outside the reservation window.
    static DIRECTORY_SERVED: Cell<u32> = const { Cell::new(0) };
    /// How many introspection reads were refused.
    static INTROSPECTION_REFUSED: Cell<u32> = const { Cell::new(0) };
}

/// A global allocator that routes an allocation through a blind pool on every
/// global allocation the pool makes while armed.
#[cfg(not(miri))]
struct ReentrantGlobalAlloc;

// SAFETY: every request is forwarded to `System`. The probe is re-entrance
// guarded and returns normally, so no unwind escapes a callback.
#[cfg(not(miri))]
unsafe impl GlobalAlloc for ReentrantGlobalAlloc {
    unsafe fn alloc(&self, layout: StdLayout) -> *mut u8 {
        probe();
        // SAFETY: forwarded unchanged.
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: StdLayout) {
        // SAFETY: forwarded unchanged from a block `System` produced.
        unsafe { System.dealloc(ptr, layout) };
    }
}

/// Allocates from the armed pool, recording whether the request was refused.
#[cfg(not(miri))]
fn probe() {
    // `try_with` because the allocator outlives thread-local destruction.
    let target = DIRECTORY_TARGET.try_with(Cell::get).unwrap_or(ptr::null());
    if target.is_null() || PROBING.with(|flag| flag.replace(true)) {
        return;
    }
    // SAFETY: the test arms this with a pointer to a pool that outlives the
    // armed region, so the borrow cannot dangle.
    let pool = unsafe { &*target };
    if pool.try_alloc_box(9_u8).is_err() {
        DIRECTORY_REFUSED.set(DIRECTORY_REFUSED.get() + 1);
        // Introspection has no error to return, so it panics instead. Catching
        // the unwind here keeps it from crossing the allocator callback, where
        // unwinding would be undefined behavior.
        if catch_unwind(AssertUnwindSafe(|| pool.layouts())).is_err() {
            INTROSPECTION_REFUSED.set(INTROSPECTION_REFUSED.get() + 1);
        }
    } else {
        DIRECTORY_SERVED.set(DIRECTORY_SERVED.get() + 1);
    }
    PROBING.set(false);
}

#[cfg(not(miri))]
#[global_allocator]
static GLOBAL: ReentrantGlobalAlloc = ReentrantGlobalAlloc;

#[cfg(not(miri))]
#[test]
fn routing_refuses_a_reentrant_directory_access() {
    let pool = BlindPool::new();
    // The probe allocates this layout, so installing it up front keeps the
    // probe itself from perturbing the directory it is meant to observe.
    let probe_layout = pool.alloc_box(0_u8);

    DIRECTORY_TARGET.set(&raw const pool);
    // Each distinct layout installs a pool, and the directory reallocates as it
    // outgrows its capacity — which is the window under test.
    let boxes = (
        pool.alloc_box([0_u8; 2]),
        pool.alloc_box([0_u8; 3]),
        pool.alloc_box([0_u8; 4]),
        pool.alloc_box([0_u8; 5]),
        pool.alloc_box([0_u8; 6]),
        pool.alloc_box([0_u8; 7]),
        pool.alloc_box([0_u8; 8]),
        pool.alloc_box([0_u8; 9]),
    );
    DIRECTORY_TARGET.set(ptr::null());

    assert!(
        DIRECTORY_REFUSED.get() > 0,
        "a nested routing request during directory reservation must be refused"
    );
    // Allocations outside the reservation window are served, which is what
    // makes the refusals above attributable to the window itself.
    assert!(DIRECTORY_SERVED.get() > 0, "reentrant routing outside the window must still work");
    assert!(
        INTROSPECTION_REFUSED.get() > 0,
        "a directory read from inside the reservation window must panic"
    );

    // The directory is intact: every layout is installed exactly once and every
    // value survived the refusals.
    assert_eq!(*probe_layout, 0);
    assert_eq!(boxes.0.len(), 2);
    assert_eq!(boxes.7.len(), 9);
    assert_eq!(pool.layouts(), 9);
}
