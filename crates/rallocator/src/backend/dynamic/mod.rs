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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
enum Choice {
    // Zero is reserved for the uninitialized cache.
    Snmalloc = 1,
    Mimalloc,
    System,
}

static SELECTED: AtomicU8 = AtomicU8::new(0);

#[inline]
fn selected(cache: &AtomicU8, on_first_use: impl FnOnce(&AtomicU8) -> Choice) -> Choice {
    // A single relaxed load replaces a OnceLock readiness check and a separate
    // load of its value. The tag is the only data we publish.
    match cache.load(Ordering::Relaxed) {
        tag if tag == Choice::Snmalloc as u8 => Choice::Snmalloc,
        tag if tag == Choice::Mimalloc as u8 => Choice::Mimalloc,
        tag if tag == Choice::System as u8 => Choice::System,
        _ => on_first_use(cache),
    }
}

#[cold]
fn from_environment(cache: &AtomicU8) -> Choice {
    initialize(cache, env::read, env::stderr_warning(env::write_warning))
}

fn initialize(cache: &AtomicU8, read: impl FnOnce() -> Option<Choice>, warn: impl FnOnce()) -> Choice {
    let configured = read();
    let choice = configured.unwrap_or(Choice::Snmalloc);
    // Racing first callers can read the environment, but only the winner's
    // choice can ever own a block. No caller waits for another initializer.
    if cache
        .compare_exchange(0, choice as u8, Ordering::Relaxed, Ordering::Relaxed)
        .is_err()
    {
        return selected(cache, from_environment);
    }
    if configured.is_none() {
        warn();
    }
    choice
}

// SAFETY: The process cache selects the same backend for every operation.
unsafe impl Operations for Dynamic {
    #[inline]
    unsafe fn allocate<const ZEROED: bool>(&self, layout: Layout) -> *mut u8 {
        // SAFETY: The caller supplies a valid nonzero layout.
        unsafe { selected(&SELECTED, from_environment).allocate::<ZEROED>(layout) }
    }

    #[inline]
    unsafe fn deallocate(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: The selected backend owns the block with this layout.
        unsafe { selected(&SELECTED, from_environment).deallocate(ptr, layout) };
    }

    #[inline]
    unsafe fn reallocate(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: The owning backend receives the caller's valid resize.
        unsafe { selected(&SELECTED, from_environment).reallocate(ptr, layout, new_size) }
    }

    unsafe fn reallocate_recorded(&self, ptr: *mut u8, layout: Layout, new_size: usize, record: impl FnOnce(*mut u8)) -> *mut u8 {
        // SAFETY: The owning backend handles the resize and the callback together.
        unsafe { selected(&SELECTED, from_environment).reallocate_recorded(ptr, layout, new_size, record) }
    }

    fn publish_state(&self) {
        selected(&SELECTED, from_environment).publish_state();
    }
}

#[expect(
    clippy::multiple_unsafe_ops_per_block,
    reason = "Each match forwards one unsafe operation with the same contract in every arm"
)]
// SAFETY: Each choice forwards to one allocator, including retirement after a recorded resize.
unsafe impl Operations for Choice {
    #[inline]
    unsafe fn allocate<const ZEROED: bool>(&self, layout: Layout) -> *mut u8 {
        // SAFETY: All three allocators receive the caller's valid nonzero layout.
        unsafe {
            match self {
                Self::Snmalloc => Snmalloc.allocate::<ZEROED>(layout),
                Self::Mimalloc => Mimalloc.allocate::<ZEROED>(layout),
                Self::System => System.allocate::<ZEROED>(layout),
            }
        }
    }

    #[inline]
    unsafe fn deallocate(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: Selection is immutable, so the live block returns to its creator.
        unsafe {
            match self {
                Self::Snmalloc => Snmalloc.deallocate(ptr, layout),
                Self::Mimalloc => Mimalloc.deallocate(ptr, layout),
                Self::System => System.dealloc(ptr, layout),
            }
        }
    }

    #[inline]
    unsafe fn reallocate(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: The creating backend receives the original layout and valid new size.
        unsafe {
            match self {
                Self::Snmalloc => Snmalloc.reallocate(ptr, layout, new_size),
                Self::Mimalloc => Mimalloc.reallocate(ptr, layout, new_size),
                Self::System => System.reallocate(ptr, layout, new_size),
            }
        }
    }

    unsafe fn reallocate_recorded(&self, ptr: *mut u8, layout: Layout, new_size: usize, record: impl FnOnce(*mut u8)) -> *mut u8 {
        // SAFETY: The creating backend handles the full resize, including retirement after the callback.
        unsafe {
            match self {
                Self::Snmalloc => Snmalloc.reallocate_recorded(ptr, layout, new_size, record),
                Self::Mimalloc => Mimalloc.reallocate_recorded(ptr, layout, new_size, record),
                Self::System => System.reallocate_recorded(ptr, layout, new_size, record),
            }
        }
    }

    fn publish_state(&self) {
        match self {
            Self::Snmalloc => Snmalloc.publish_state(),
            Self::Mimalloc => Mimalloc.publish_state(),
            Self::System => {}
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

#[cfg(test)]
mod tests;
