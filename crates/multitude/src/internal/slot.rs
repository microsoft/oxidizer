// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Write-once `UninitSlot<T>` for reserved but uninitialized memory.
//!
//! Construction is `unsafe`; once created, `write_unaligned` is safe and
//! consumes the slot, preventing double writes.

use core::marker::PhantomData;
use core::ptr::NonNull;

/// A non-null, exclusively owned `T` slot that has not been initialized.
pub(crate) struct UninitSlot<T: ?Sized> {
    ptr: NonNull<T>,
    /// `*mut T` keeps this as raw memory, not a borrow.
    _marker: PhantomData<*mut T>,
}

impl<T> UninitSlot<T> {
    /// Wrap an exclusively owned, uninitialized `T` slot.
    ///
    /// # Safety
    ///
    /// `ptr` must be non-null, aligned for `T`, address
    /// `size_of::<T>()` writable bytes, and point to memory that is
    /// uninitialized or can be overwritten without running `Drop`.
    #[inline]
    pub(crate) const unsafe fn new(ptr: NonNull<T>) -> Self {
        Self { ptr, _marker: PhantomData }
    }

    /// Like [`Self::new`], but takes a raw pointer.
    ///
    /// # Safety
    ///
    /// Same as [`Self::new`], plus `ptr` must be non-null.
    #[inline]
    pub(crate) const unsafe fn from_raw(ptr: *mut T) -> Self {
        // SAFETY: caller asserts non-null.
        let nn = unsafe { NonNull::new_unchecked(ptr) };
        // SAFETY: caller forwards the slot invariants.
        unsafe { Self::new(nn) }
    }

    /// Write `value` with an unaligned store.
    ///
    /// This is safe because `UninitSlot` carries the addressability and
    /// exclusivity invariants, and `write_unaligned` does not require
    /// alignment.
    #[inline]
    pub(crate) fn write_unaligned(self, value: T) -> NonNull<T> {
        // SAFETY: addressable + exclusive + uninitialized.
        unsafe { core::ptr::write_unaligned(self.ptr.as_ptr(), value) };
        self.ptr
    }
}

#[cfg(test)]
mod tests {
    use super::UninitSlot;

    #[test]
    fn write_unaligned_initializes_slot() {
        let mut bytes = [0_u8; 16];
        // Use an off-by-one address to hit the unaligned path.
        // SAFETY: offset 1 stays in-bounds, and the pointer is only
        // used with unaligned access below.
        #[allow(
            clippy::cast_ptr_alignment,
            reason = "intentionally misaligned to exercise UninitSlot::write_unaligned"
        )]
        // SAFETY: 16-byte buffer; offset 1 is in-bounds.
        let ptr: *mut u64 = unsafe { bytes.as_mut_ptr().add(1).cast::<u64>() };
        // SAFETY: bytes 1..9 are exclusively owned and unread until
        // after `write_unaligned`.
        let slot = unsafe { UninitSlot::<u64>::from_raw(ptr) };
        let _ = slot.write_unaligned(0x0102_0304_0506_0708);
        // SAFETY: bytes 1..9 were just initialized by `write_unaligned`.
        let read = unsafe { core::ptr::read_unaligned(ptr) };
        assert_eq!(read, 0x0102_0304_0506_0708);
    }
}
