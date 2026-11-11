// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.
// Derived from mimalloc internal.h and arena.c; see LICENSE-MIMALLOC.

//! Aligned page metadata for the non-hardened allocator.
//!
//! Each 256-MiB region starts with a metadata slot for each 64-KiB slice.
//! A slot's atomic self pointer identifies the actual page. Small pages keep
//! their complete metadata at the start of their data slice.

use std::sync::atomic::{AtomicUsize, Ordering};

use crate::backend::mimalloc::classes::SLICE;
use crate::backend::mimalloc::heap::{PAGE_META_SIZE, PAGE_SELF_OFFSET};

pub(super) const REGION_SIZE: usize = 4096 * SLICE;

#[inline]
fn slot(address: usize) -> *const AtomicUsize {
    let region = address & !(REGION_SIZE - 1);
    let slice = (address / SLICE) % (REGION_SIZE / SLICE);
    std::ptr::with_exposed_provenance(region + slice * PAGE_META_SIZE + PAGE_SELF_OFFSET)
}

/// # Safety
/// The range is exclusively owned. Its aligned metadata slots are committed,
/// initialized to zero or previously unregistered, and not concurrently used.
/// The page is initialized and stays live until the range is unregistered.
pub(super) unsafe fn register(address: usize, size: usize, page: usize) {
    for offset in (0..size).step_by(SLICE) {
        // SAFETY: Each slot is committed atomic storage belonging to this range.
        unsafe { (*slot(address + offset)).store(page, Ordering::Release) };
    }
}

/// # Safety
/// The range is registered and all its blocks and bitmap readers are retired.
pub(super) unsafe fn unregister(address: usize, size: usize) {
    for offset in (0..size).step_by(SLICE) {
        // SAFETY: The caller owns every registered slot and has retired its readers.
        unsafe { (*slot(address + offset)).store(0, Ordering::Release) };
    }
}

/// # Safety
/// The address belongs to a live block, or is pinned by an abandoned bitmap
/// reader. The allocation pins its committed metadata slot and actual page.
#[inline]
pub(super) unsafe fn lookup(address: usize) -> usize {
    // SAFETY: A live registered block or reader pins its initialized metadata.
    unsafe { (*slot(address)).load(Ordering::Acquire) }
}
