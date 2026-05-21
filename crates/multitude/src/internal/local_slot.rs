// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! `LocalSlot<T>` is an `UnsafeCell<T>` with a manual `Sync` impl for
//! state that is only touched from one thread by external invariant.

use core::cell::UnsafeCell;

/// A cell that is treated as `Sync` under a single-threaded access invariant.
///
/// Every call to [`Self::with_mut`] must happen on the owning thread.
pub(crate) struct LocalSlot<T>(UnsafeCell<T>);

impl<T> LocalSlot<T> {
    #[inline]
    pub(crate) const fn new(value: T) -> Self {
        Self(UnsafeCell::new(value))
    }

    /// Run `f` with exclusive access to the value.
    ///
    /// # Safety
    ///
    /// - No other thread may be accessing this slot.
    /// - `f` must not make a reentrant call to `with_mut` on the
    ///   **same** slot — that would create overlapping `&mut T`
    ///   borrows (UB under both Stacked and Tree Borrows). `f` may
    ///   borrow other `LocalSlot`s freely.
    #[inline]
    pub(crate) unsafe fn with_mut<R>(&self, f: impl FnOnce(&mut T) -> R) -> R {
        // SAFETY: caller guarantees single-thread-local access and
        // no reentrant `with_mut` on the same slot.
        let r = unsafe { &mut *self.0.get() };
        f(r)
    }
}

// SAFETY: only the arena's owning thread calls `with_mut`.
unsafe impl<T> Sync for LocalSlot<T> {}
