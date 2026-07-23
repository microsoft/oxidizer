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
