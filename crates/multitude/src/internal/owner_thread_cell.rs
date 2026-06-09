// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Owner-thread-confined cell: shared-Sync wrapper around an `UnsafeCell`.
//!
//! The cell is `Sync` so it can live inside a struct that is itself shared
//! across threads, but every access goes through `unsafe fn with`. The
//! `unsafe` caller asserts that the call happens on the cell's logical
//! "owner thread"; concurrent access is undefined behavior.
//!
//! Used by [`ChunkProvider`](super::ChunkProvider) to hold the local-chunk
//! cache head and local high-water mark — both touched exclusively by the
//! arena's owning thread, even though the provider itself is `Sync`.

use core::cell::UnsafeCell;

/// `UnsafeCell<T>` with a manually-asserted owner-thread invariant.
pub(crate) struct OwnerThreadCell<T> {
    inner: UnsafeCell<T>,
}

// SAFETY: All access is through `with`, whose `unsafe` contract requires the
// caller to be on the cell's owner thread. We make no claim about concurrent
// access — that obligation is fully delegated to the caller.
unsafe impl<T: Send> Sync for OwnerThreadCell<T> {}

impl<T> OwnerThreadCell<T> {
    /// Creates a new cell holding `value`.
    #[inline]
    pub(crate) const fn new(value: T) -> Self {
        Self {
            inner: UnsafeCell::new(value),
        }
    }

    /// Runs `f` with exclusive access to the cell's contents.
    ///
    /// # Safety
    ///
    /// The caller must guarantee that this call is on the cell's owner
    /// thread and that no other access (read or write) to the cell is in
    /// flight on any thread for the duration of the call. The body of `f`
    /// must not call back into `with` on the same cell.
    #[inline]
    pub(crate) unsafe fn with<R>(&self, f: impl FnOnce(&mut T) -> R) -> R {
        // SAFETY: per the function's safety contract, no other reference to
        // the cell's contents exists for the duration of the call, so the
        // `&mut T` reborrow is exclusive.
        let r = unsafe { &mut *self.inner.get() };
        f(r)
    }
}
