// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::alloc::Layout;
use std::sync::atomic::{AtomicU8, Ordering};

use super::sealed::Operations;
use super::{Mimalloc, Snmalloc, System};

mod env;

/// Selects a backend at startup for [`rallocator!(backend = dynamic)`](crate::rallocator).
///
/// The backend `D` serves the process if `RALLOCATOR_DYNAMIC_PERSONALITY`
/// is unset, empty, or unknown.
#[derive(Clone, Copy, Debug, Default)]
#[doc(hidden)]
pub struct Dynamic<D = System>(pub D);

impl<D: Fixed> super::Backend for Dynamic<D> {}

/// A fixed backend that [`Dynamic`] can use as its default.
pub trait Fixed {
    /// The cache tag of this backend.
    const CHOICE: Choice;
}

impl Fixed for System {
    const CHOICE: Choice = Choice::System;
}

impl Fixed for Snmalloc {
    const CHOICE: Choice = Choice::Snmalloc;
}

impl Fixed for Mimalloc {
    const CHOICE: Choice = Choice::Mimalloc;
}

/// A fixed backend that the process cache can hold.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum Choice {
    // Zero is reserved for the uninitialized cache.
    Snmalloc = 1,
    Mimalloc,
    System,
}

/// A parsed `RALLOCATOR_DYNAMIC_PERSONALITY` value.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Setting {
    /// The variable is unset or empty.
    Unset,
    /// The variable names a backend.
    Known(Choice),
    /// The variable has any other value.
    Unknown,
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
fn from_environment(cache: &AtomicU8, default: Choice) -> Choice {
    initialize(cache, default, env::read, env::stderr_warning(env::write_warning))
}

fn initialize(cache: &AtomicU8, default: Choice, read: impl FnOnce() -> Setting, warn: impl FnOnce(Choice)) -> Choice {
    let setting = read();
    let choice = match setting {
        Setting::Known(choice) => choice,
        Setting::Unset | Setting::Unknown => default,
    };
    // Racing first callers can read the environment, but only the winner's
    // choice can ever own a block. No caller waits for another initializer.
    if cache
        .compare_exchange(0, choice as u8, Ordering::Relaxed, Ordering::Relaxed)
        .is_err()
    {
        return selected(cache, |cache| from_environment(cache, default));
    }
    if setting == Setting::Unknown {
        warn(choice);
    }
    choice
}

impl<D: Fixed> Dynamic<D> {
    #[inline]
    fn backend() -> Choice {
        selected(&SELECTED, |cache| from_environment(cache, D::CHOICE))
    }
}

// SAFETY: One process cache selects the backend for every operation of every
// `Dynamic` handle, whatever its default.
unsafe impl<D: Fixed> Operations for Dynamic<D> {
    #[inline]
    unsafe fn allocate<const ZEROED: bool>(&self, layout: Layout) -> *mut u8 {
        // SAFETY: The caller supplies a valid nonzero layout.
        unsafe { Self::backend().allocate::<ZEROED>(layout) }
    }

    #[inline]
    unsafe fn deallocate(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: The selected backend owns the block with this layout.
        unsafe { Self::backend().deallocate(ptr, layout) };
    }

    #[inline]
    unsafe fn reallocate(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: The owning backend receives the caller's valid resize.
        unsafe { Self::backend().reallocate(ptr, layout, new_size) }
    }

    unsafe fn reallocate_recorded(&self, ptr: *mut u8, layout: Layout, new_size: usize, record: impl FnOnce(*mut u8)) -> *mut u8 {
        // SAFETY: The owning backend handles the resize and the callback together.
        unsafe { Self::backend().reallocate_recorded(ptr, layout, new_size, record) }
    }

    fn publish_state(&self) {
        Self::backend().publish_state();
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
                Self::System => System.deallocate(ptr, layout),
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
            Self::System => System.publish_state(),
        }
    }
}

#[cfg(test)]
mod tests;
