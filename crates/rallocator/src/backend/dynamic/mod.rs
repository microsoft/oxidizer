// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicU8, Ordering};

use super::sealed::Operations;
use super::{Mimalloc, Snmalloc};

mod env;

/// Selects a backend at startup for [`rallocator!(backend = dynamic)`](crate::rallocator).
#[derive(Clone, Copy, Debug, Default)]
#[doc(hidden)]
pub struct Dynamic;

impl super::Backend for Dynamic {}

#[derive(Clone, Copy)]
#[repr(u8)]
enum Choice {
    // Zero is reserved for the uninitialized cache.
    Snmalloc = 1,
    Mimalloc,
    System,
}

static SELECTED: AtomicU8 = AtomicU8::new(0);

#[inline]
fn selected() -> Choice {
    // A single relaxed load replaces a OnceLock readiness check and a separate
    // load of its value. The tag is the only data we publish.
    match SELECTED.load(Ordering::Relaxed) {
        tag if tag == Choice::Snmalloc as u8 => Choice::Snmalloc,
        tag if tag == Choice::Mimalloc as u8 => Choice::Mimalloc,
        tag if tag == Choice::System as u8 => Choice::System,
        _ => initialize(),
    }
}

#[cold]
fn initialize() -> Choice {
    let configured = env::read();
    let choice = configured.unwrap_or(Choice::Snmalloc);
    // Racing first callers can read the environment, but only the winner's
    // choice can ever own a block. No caller waits for another initializer.
    if SELECTED
        .compare_exchange(0, choice as u8, Ordering::Relaxed, Ordering::Relaxed)
        .is_err()
    {
        return selected();
    }
    if configured.is_none() {
        env::warn_invalid();
    }
    choice
}

#[expect(
    clippy::multiple_unsafe_ops_per_block,
    reason = "Each match forwards one unsafe operation with the same contract in every arm"
)]
// SAFETY: Every operation forwards to the same backend for the process lifetime.
// System uses its GlobalAlloc contract except when a recorded resize must keep
// the original block live until the facade records the move.
unsafe impl Operations for Dynamic {
    #[inline]
    unsafe fn allocate<const ZEROED: bool>(&self, layout: Layout) -> *mut u8 {
        // SAFETY: All three allocators receive the caller's valid nonzero layout.
        unsafe {
            match selected() {
                Choice::Snmalloc => Snmalloc.allocate::<ZEROED>(layout),
                Choice::Mimalloc => Mimalloc.allocate::<ZEROED>(layout),
                Choice::System => System.allocate::<ZEROED>(layout),
            }
        }
    }

    #[inline]
    unsafe fn deallocate(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: Selection is immutable, so the live block returns to its creator.
        unsafe {
            match selected() {
                Choice::Snmalloc => Snmalloc.deallocate(ptr, layout),
                Choice::Mimalloc => Mimalloc.deallocate(ptr, layout),
                Choice::System => System.dealloc(ptr, layout),
            }
        }
    }

    #[inline]
    unsafe fn reallocate(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: The creating backend receives the original layout and valid new size.
        unsafe {
            match selected() {
                Choice::Snmalloc => Snmalloc.reallocate(ptr, layout, new_size),
                Choice::Mimalloc => Mimalloc.reallocate(ptr, layout, new_size),
                Choice::System => System.reallocate(ptr, layout, new_size),
            }
        }
    }

    unsafe fn reallocate_recorded(&self, ptr: *mut u8, layout: Layout, new_size: usize, record: impl FnOnce(*mut u8)) -> *mut u8 {
        // SAFETY: The creating backend handles the full resize, including retirement after the callback.
        unsafe {
            match selected() {
                Choice::Snmalloc => Snmalloc.reallocate_recorded(ptr, layout, new_size, record),
                Choice::Mimalloc => Mimalloc.reallocate_recorded(ptr, layout, new_size, record),
                Choice::System => System.reallocate_recorded(ptr, layout, new_size, record),
            }
        }
    }

    fn publish_state(&self) {
        match selected() {
            Choice::Snmalloc => Snmalloc.publish_state(),
            Choice::Mimalloc => Mimalloc.publish_state(),
            Choice::System => {}
        }
    }
}

// SAFETY: System owns every block. Resize preparation allocates disjoint storage
// without releasing the original, as required by the recorded resize protocol.
unsafe impl Operations for System {
    unsafe fn allocate<const ZEROED: bool>(&self, layout: Layout) -> *mut u8 {
        if ZEROED {
            // SAFETY: The caller supplies a valid nonzero allocation layout.
            unsafe { self.alloc_zeroed(layout) }
        } else {
            // SAFETY: The same layout is valid for an uninitialized allocation.
            unsafe { self.alloc(layout) }
        }
    }

    unsafe fn deallocate(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: The caller supplies a live System block and its exact layout.
        unsafe { self.dealloc(ptr, layout) };
    }

    unsafe fn reallocate(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: The live System block, layout and new size meet GlobalAlloc's resize contract.
        unsafe { self.realloc(ptr, layout, new_size) }
    }

    fn publish_state(&self) {}
}
