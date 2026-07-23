// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Growable, arena-backed buffer of `T`.
//!
//! Backing storage for `Vec<'a, T, A>`, `String<'a, A>`, and
//! `Utf16String<'a, A>`. Growth is mediated by
//! [`ChunkMutator`](super::chunk_mutator::ChunkMutator); this file owns the unsafe
//! `(ptr, len, cap)` invariant.

use core::iter::FusedIterator;
use core::marker::PhantomData;
use core::ptr::{self, NonNull};
use core::{mem, slice};

use super::constants::buffer_freezable;
use super::uninit::{Uninit, copy_bytes_nonoverlapping};

/// A growable buffer of `T` whose storage lives in an arena chunk.
///
/// # Invariants
///
/// - `ptr` is non-null and well-aligned for `T`. When `cap == 0` (or for
///   ZSTs) it is permitted to be dangling.
/// - When `cap > 0` and `T` is non-ZST, `ptr` addresses `cap` consecutive
///   `T` slots in a live arena chunk that outlives `'a`.
/// - The first `len` of those slots are initialized; the rest are not.
/// - `len <= cap`.
pub(crate) struct ArenaBuf<'a, T> {
    ptr: NonNull<T>,
    len: usize,
    cap: usize,
    /// Whether `ptr` is immediately preceded by an `Arc<[T]>` freeze prefix
    /// (`[strong][pad][len]`), letting the buffer freeze into an `Arc<[T]>` /
    /// `Box<[T]>` in place. True for buffers installed by the freezable
    /// reservation path; false for empty buffers and `split_off` tails (whose
    /// base points mid-chunk, with no prefix of their own).
    freeze_prefix: bool,
    _phantom: PhantomData<&'a mut [T]>,
}

impl<T> ArenaBuf<'_, T> {
    /// Reconstruct a buffer from raw parts.
    ///
    /// # Safety
    ///
    /// `(ptr, len, cap)` must satisfy the type invariants for storage in a
    /// live arena chunk that outlives `'a`. `freeze_prefix` must be true only
    /// when `ptr` is immediately preceded by a valid `Arc<[T]>` freeze prefix.
    #[inline]
    pub(crate) const unsafe fn from_raw_parts(ptr: NonNull<T>, len: usize, cap: usize, freeze_prefix: bool) -> Self {
        Self {
            ptr,
            len,
            cap,
            freeze_prefix,
            _phantom: PhantomData,
        }
    }
}

impl<'a, T> ArenaBuf<'a, T> {
    /// Creates an empty buffer. ZST buffers use `cap = usize::MAX`
    /// because they need no backing storage.
    #[inline]
    pub(crate) const fn new() -> Self {
        let cap = if mem::size_of::<T>() == 0 { usize::MAX } else { 0 };
        Self {
            ptr: NonNull::dangling(),
            len: 0,
            cap,
            freeze_prefix: false,
            _phantom: PhantomData,
        }
    }

    /// Whether this buffer carries the `Arc<[T]>` freeze prefix and can be
    /// frozen into an `Arc<[T]>` / `Box<[T]>` in place (no copy).
    #[inline]
    pub(crate) const fn has_freeze_prefix(&self) -> bool {
        self.freeze_prefix
    }

    /// Returns the current number of initialized elements.
    #[inline]
    pub(crate) const fn len(&self) -> usize {
        self.len
    }

    /// Returns the total capacity in elements.
    #[inline]
    pub(crate) const fn cap(&self) -> usize {
        self.cap
    }

    /// Returns the number of additional elements that can be pushed
    /// without growing.
    #[inline]
    pub(crate) const fn remaining_cap(&self) -> usize {
        self.cap - self.len
    }

    /// Returns the raw const pointer to the buffer's start.
    #[inline]
    pub(crate) const fn as_ptr(&self) -> *const T {
        self.ptr.as_ptr()
    }

    /// Returns the raw mutable pointer to the buffer's start.
    #[expect(
        clippy::needless_pass_by_ref_mut,
        reason = "mutable self required for API safety: callers must hold exclusive access to obtain a mutable pointer"
    )]
    #[inline]
    pub(crate) const fn as_mut_ptr(&mut self) -> *mut T {
        self.ptr.as_ptr()
    }

    /// Returns the initialized prefix as a slice.
    #[inline]
    pub(crate) fn as_slice(&self) -> &[T] {
        // SAFETY: by the type's invariants, `ptr` addresses storage of at
        // least `len` initialized `T`s (dangling for ZSTs / empty, which
        // `from_raw_parts` accepts when `len == 0` or when `T` is a ZST).
        // The `&self` borrow grants shared access.
        unsafe { slice::from_raw_parts(self.ptr.as_ptr(), self.len) }
    }

    /// Returns the initialized prefix as a mutable slice.
    #[inline]
    pub(crate) fn as_mut_slice(&mut self) -> &mut [T] {
        // SAFETY: by the type's invariants, `ptr` addresses storage of at least
        // `len` initialized `T`s (dangling for ZSTs / empty, which
        // `from_raw_parts_mut` accepts when `len == 0` or `T` is a ZST). The
        // `&mut self` borrow grants exclusive access to the slice.
        unsafe { slice::from_raw_parts_mut(self.ptr.as_ptr(), self.len) }
    }

    /// Push `value` if there is unused capacity; otherwise return it
    /// back to the caller so it can grow and retry.
    #[inline]
    pub(crate) fn push_within_cap(&mut self, value: T) -> Result<(), T> {
        if self.len == self.cap {
            return Err(value);
        }
        // SAFETY: `len < cap`, so the slot at `len` is in-bounds and
        // uninitialized (type invariant). `ptr::write` does not drop a
        // prior value. For ZSTs `ptr` may be dangling but `ptr::write`
        // with a ZST does not access memory.
        unsafe {
            ptr::write(self.ptr.as_ptr().add(self.len), value);
        }
        self.len += 1;
        Ok(())
    }

    /// Pop the last initialized element, transferring ownership to the
    /// caller. Returns `None` if the buffer is empty.
    #[inline]
    pub(crate) fn pop(&mut self) -> Option<T> {
        if self.len == 0 {
            return None;
        }
        self.len -= 1;
        // SAFETY: the slot at the (now-decremented) `len` was initialized
        // by an earlier push. Lowering `len` first ensures `Drop` won't
        // re-drop it. For ZSTs the dangling pointer is fine because
        // `ptr::read` of a ZST does not access memory.
        Some(unsafe { ptr::read(self.ptr.as_ptr().add(self.len)) })
    }

    /// Adopt a fresh slice reservation as the new backing storage,
    /// moving any existing initialized elements into it. The old
    /// buffer is abandoned (the arena reclaims it on teardown).
    #[inline]
    #[cfg_attr(test, mutants::skip)] // `>` vs `>=` equivalent: copy(0) is a no-op
    pub(crate) fn replace_buffer(&mut self, fresh: Uninit<'a, [T]>) {
        let (new_ptr, new_cap) = fresh.into_raw_buffer();
        // SAFETY: `fresh` was just consumed; the raw `(ptr, cap)` it
        // produced satisfies `replace_buffer_raw`'s contract.
        unsafe { self.replace_buffer_raw(new_ptr, new_cap) };
    }

    /// Raw-pointer variant of [`Self::replace_buffer`] for oversized
    /// growth through a temporary [`ChunkMutator`](super::chunk_mutator::ChunkMutator).
    ///
    /// # Safety
    ///
    /// `(new_ptr, new_cap)` must reference a fresh, non-overlapping
    /// reservation of at least `new_cap >= self.len` uninitialized `T`
    /// slots whose backing storage outlives `'a`.
    #[inline]
    pub(crate) unsafe fn replace_buffer_raw(&mut self, new_ptr: NonNull<T>, new_cap: usize) {
        debug_assert!(new_cap >= self.len, "replace_buffer_raw: new capacity below live length");
        if self.len > 0 {
            // SAFETY: source holds `self.len` initialized `T`s; destination
            // is a fresh, non-overlapping reservation of at least `new_cap
            // >= self.len` uninitialized `T` slots. We move ownership of
            // the elements (the old buffer is abandoned without re-drop;
            // the arena reclaims it at teardown).
            unsafe {
                ptr::copy_nonoverlapping(self.ptr.as_ptr(), new_ptr.as_ptr(), self.len);
            }
        }
        self.ptr = new_ptr;
        self.cap = new_cap;
        // The freezable reservation path installs a prefix exactly when `T`
        // is freezable; `try_grow_to` gates its reservation on the same
        // predicate, so the flag tracks the buffer that was actually placed.
        self.freeze_prefix = buffer_freezable::<T>();
    }

    /// Bulk-copy `src` into the uninitialized tail.
    ///
    /// Caller must ensure `self.remaining_cap() >= src.len()`.
    #[inline]
    pub(crate) fn extend_copy(&mut self, src: &[T])
    where
        T: Copy,
    {
        debug_assert!(self.remaining_cap() >= src.len(), "extend_copy: insufficient capacity");
        // SAFETY: the tail `[len .. len + src.len()]` is in-bounds uninit
        // storage (invariant + caller-checked capacity); `src` is a
        // caller-supplied slice and cannot alias the freshly-reserved
        // chunk storage. `T: Copy` permits bitwise duplication. When
        // `src.len() > 0`, the caller-checked `remaining_cap() >= src.len()`
        // forces `cap > 0`, so the destination is a live, `T`-aligned
        // allocation. When `src.len() == 0` the call is a no-op for which
        // `copy_nonoverlapping` only requires both pointers to be non-null
        // and `T`-aligned — satisfied because `self.ptr` is a `NonNull`
        // that `ArenaBuf` keeps `T`-aligned even while dangling (`cap == 0`
        // / ZST), and `src.as_ptr()` is likewise non-null and aligned.
        unsafe {
            let dst = self.ptr.as_ptr().add(self.len);
            if const { mem::size_of::<T>() == 1 } {
                copy_bytes_nonoverlapping(src.as_ptr().cast(), dst.cast(), src.len());
            } else {
                ptr::copy_nonoverlapping(src.as_ptr(), dst, src.len());
            }
        }
        self.len += src.len();
    }

    /// Drop all initialized elements but keep the buffer allocated.
    #[inline]
    pub(crate) fn clear(&mut self) {
        self.truncate(0);
    }

    /// Drop trailing elements so that exactly `new_len` initialized
    /// elements remain. No-op when `new_len >= len`.
    #[inline]
    pub(crate) fn truncate(&mut self, new_len: usize) {
        if new_len >= self.len {
            return;
        }
        let drop_count = self.len - new_len;
        // Lower `len` first so a panic in an element's `Drop` won't
        // leave the remainder being re-dropped twice.
        self.len = new_len;
        if const { mem::needs_drop::<T>() } {
            // SAFETY: the `drop_count` slots starting at `new_len` were
            // initialized; we hold `&mut self`, so there are no outstanding
            // references.
            unsafe {
                ptr::drop_in_place(ptr::slice_from_raw_parts_mut(self.ptr.as_ptr().add(new_len), drop_count));
            }
        }
    }

    /// O(1) remove of the element at `idx`, swapping in the last
    /// element. Returns `None` if `idx >= len`.
    #[inline]
    pub(crate) fn swap_remove(&mut self, idx: usize) -> Option<T> {
        if idx >= self.len {
            return None;
        }
        let last = self.len - 1;
        if idx != last {
            self.as_mut_slice().swap(idx, last);
        }
        self.pop()
    }

    /// Shift-insert: insert `value` at `idx`, sliding elements
    /// `[idx..len]` right by one. Caller must ensure `idx <= len` and
    /// `remaining_cap() >= 1`.
    #[inline]
    #[cfg_attr(test, mutants::skip)] // guard/arithmetic mutations are observationally equivalent for in-cap writes
    pub(crate) fn insert_within_cap(&mut self, idx: usize, value: T) {
        debug_assert!(idx <= self.len, "insert_within_cap: idx out of bounds");
        debug_assert!(self.remaining_cap() >= 1, "insert_within_cap: no space");
        // SAFETY: `idx <= len < cap` so both source `[idx, len)` and
        // destination `[idx+1, len+1)` are in-bounds. `ptr::copy` handles
        // overlap. The slot at `idx` is then uninitialized for `ptr::write`.
        // For ZSTs the dangling pointer is fine (no memory access).
        unsafe {
            let base = self.ptr.as_ptr().add(idx);
            if idx < self.len {
                ptr::copy(base, base.add(1), self.len - idx);
            }
            ptr::write(base, value);
        }
        self.len += 1;
    }

    /// Shift-remove the element at `idx`, sliding `[idx+1..len]` left
    /// by one and returning the removed value. Returns `None` when
    /// `idx >= len`.
    #[inline]
    #[cfg_attr(test, mutants::skip)] // tail-shift mutations stay within abandoned slots
    pub(crate) fn remove(&mut self, idx: usize) -> Option<T> {
        if idx >= self.len {
            return None;
        }
        // SAFETY: `idx < len` so the slot is initialized; `ptr::read`
        // transfers ownership. The subsequent `ptr::copy` shifts the
        // suffix `[idx+1, len)` down by one over an overlap-safe move.
        // Lowering `len` afterward keeps the moved-from tail from being
        // seen as initialized.
        let value = unsafe {
            let base = self.ptr.as_ptr().add(idx);
            let value = ptr::read(base);
            let tail = self.len - idx - 1;
            if tail > 0 {
                ptr::copy(base.add(1), base, tail);
            }
            value
        };
        self.len -= 1;
        Some(value)
    }

    /// Force-set `len`. Caller is responsible for ensuring the prefix
    /// `[..new_len]` is initialized and `new_len <= cap`.
    ///
    /// # Safety
    ///
    /// `new_len <= cap` and slots `[..new_len]` must be initialized.
    #[inline]
    pub(crate) const unsafe fn set_len(&mut self, new_len: usize) {
        debug_assert!(new_len <= self.cap, "set_len: new_len exceeds cap");
        self.len = new_len;
    }

    /// Splits the buffer at `at`, keeping `[0, at)` in `self` and
    /// returning a new buffer that owns `[at, len)`.
    ///
    /// No elements are copied. `self.cap` is capped at `at`, and the returned
    /// tail owns the remaining capacity in the same arena chunk.
    ///
    /// Caller must ensure `at <= len`.
    #[inline]
    pub(crate) fn split_off_buf(&mut self, at: usize) -> Self {
        debug_assert!(at <= self.len, "split_off_buf: at exceeds len");
        let tail_len = self.len - at;
        if const { mem::size_of::<T>() == 0 } {
            self.len = at;
            return ArenaBuf {
                ptr: NonNull::dangling(),
                len: tail_len,
                cap: usize::MAX,
                freeze_prefix: false,
                _phantom: PhantomData,
            };
        }
        // SAFETY: `at <= len <= cap`, so `ptr + at` lies within the
        // original allocation (or is the one-past-the-end pointer when
        // `at == cap`), satisfying the requirements of `add`.
        let tail_ptr = unsafe { NonNull::new_unchecked(self.ptr.as_ptr().add(at)) };
        let tail_cap = self.cap - at;
        self.len = at;
        self.cap = at;
        ArenaBuf {
            ptr: tail_ptr,
            len: tail_len,
            cap: tail_cap,
            // The tail's base points mid-chunk: it has no freeze prefix of its
            // own (the head still owns the buffer's prefix).
            freeze_prefix: false,
            _phantom: PhantomData,
        }
    }

    /// Attempts to absorb `other`'s storage in O(1) when it directly
    /// abuts the end of `self`'s storage in the same chunk.
    ///
    /// Succeeds only when `self` is full and `other` starts at `self`'s
    /// one-past-end address. On success, `self` owns both ranges and
    /// `other` is reset to empty without dropping elements.
    #[inline]
    pub(crate) fn try_absorb_adjacent(&mut self, other: &mut Self) -> bool {
        debug_assert!(mem::size_of::<T>() != 0, "try_absorb_adjacent: not for ZSTs");
        if self.len != self.cap {
            return false;
        }
        let self_end = self.ptr.as_ptr().wrapping_add(self.cap);
        // Pointer equality proves same-chunk adjacency: another chunk's
        // payload cannot begin exactly at this chunk's one-past-end address.
        if !ptr::eq(self_end.cast_const(), other.ptr.as_ptr().cast_const()) {
            return false;
        }
        self.cap += other.cap;
        self.len += other.len;
        // Ownership of `other`'s elements has moved into `self`; reset
        // `other` so its `Drop` is a no-op and it cannot alias the storage.
        other.ptr = NonNull::dangling();
        other.len = 0;
        other.cap = 0;
        true
    }

    /// Reduce the reported capacity to `new_cap` without touching the
    /// live prefix.
    ///
    /// # Safety
    ///
    /// `new_cap >= len`, and the storage in `[new_cap, cap)` must no
    /// longer be owned by this buffer (e.g. it has been reclaimed back to
    /// the chunk's bump cursor), so that the buffer never writes there.
    #[inline]
    pub(crate) const unsafe fn set_cap(&mut self, new_cap: usize) {
        debug_assert!(new_cap >= self.len, "set_cap: new_cap below len");
        self.cap = new_cap;
    }

    /// Returns the spare capacity `[len, cap)` as a mutable slice of
    /// `MaybeUninit<T>`.
    #[inline]
    pub(crate) fn spare_capacity_mut(&mut self) -> &mut [mem::MaybeUninit<T>] {
        let spare = self.cap - self.len;
        // SAFETY: by the invariants, `ptr + len` addresses `cap - len`
        // uninitialized `T` slots; `MaybeUninit<T>` has the same layout as
        // `T`, and the `&mut self` borrow grants exclusive access. For ZSTs
        // the dangling-but-aligned pointer is valid for any length.
        unsafe {
            let ptr = self.ptr.as_ptr().add(self.len).cast::<mem::MaybeUninit<T>>();
            slice::from_raw_parts_mut(ptr, spare)
        }
    }

    /// Returns an owning iterator over the live elements and leaves the
    /// buffer empty. Dropping the iterator drops any unyielded elements.
    ///
    /// # Caller contract
    ///
    /// [`DrainAll`] is bound to arena lifetime `'a`, not the `&mut self`
    /// borrow. The caller must not touch `self` until the iterator is
    /// consumed or dropped, because the iterator owns `[0, len)`.
    #[inline]
    pub(crate) fn drain_all(&mut self) -> DrainAll<'a, T> {
        let len = self.len;
        // Move ownership of all elements out of the buffer; `len` is
        // zeroed so the buffer's own `Drop` does nothing.
        self.len = 0;
        DrainAll {
            ptr: self.ptr,
            head: 0,
            tail: len,
            _marker: PhantomData,
        }
    }
}

/// Owning iterator over an [`ArenaBuf`]'s live elements.
pub(crate) struct DrainAll<'a, T> {
    ptr: NonNull<T>,
    head: usize,
    tail: usize,
    _marker: PhantomData<&'a mut [T]>,
}

impl<T> Iterator for DrainAll<'_, T> {
    type Item = T;
    #[inline]
    #[cfg_attr(test, mutants::skip)] // `+= → *=` on `self.head` ⇒ iterator never advances ⇒ infinite loop
    fn next(&mut self) -> Option<T> {
        if self.head == self.tail {
            return None;
        }
        // SAFETY: `head < tail` and `[head, tail)` is the still-initialized
        // sub-range; advancing `head` before any further reads avoids
        // double-drop on iterator drop.
        let value = unsafe { ptr::read(self.ptr.as_ptr().add(self.head)) };
        self.head += 1;
        Some(value)
    }
    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        let n = self.tail - self.head;
        (n, Some(n))
    }
}

impl<T> DoubleEndedIterator for DrainAll<'_, T> {
    #[inline]
    fn next_back(&mut self) -> Option<T> {
        if self.head == self.tail {
            return None;
        }
        self.tail -= 1;
        // SAFETY: `head < tail` held, so `tail - 1` indexes the still-initialized
        // sub-range `[head, tail)`; reading it after decrementing `tail` moves
        // the element out and prevents a double-drop on iterator drop.
        Some(unsafe { ptr::read(self.ptr.as_ptr().add(self.tail)) })
    }
}

impl<T> ExactSizeIterator for DrainAll<'_, T> {}
impl<T> FusedIterator for DrainAll<'_, T> {}

impl<T> Drop for DrainAll<'_, T> {
    #[inline]
    #[cfg_attr(test, mutants::skip)] // mutations no-op for !Drop or empty range
    fn drop(&mut self) {
        if const { mem::needs_drop::<T>() } && self.head < self.tail {
            let len = self.tail - self.head;
            // SAFETY: `[head, tail)` is the still-initialized sub-range.
            unsafe {
                ptr::drop_in_place(ptr::slice_from_raw_parts_mut(self.ptr.as_ptr().add(self.head), len));
            }
        }
    }
}

impl<T> Drop for ArenaBuf<'_, T> {
    #[inline]
    fn drop(&mut self) {
        // Delegate to `truncate(0)`. The backing storage itself is
        // released when the owning chunk is torn down with the arena.
        self.truncate(0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Covers the ZST split_off_buf branch (lines 299-307).
    #[test]
    fn split_off_buf_zst_splits_via_len_only() {
        let mut buf: ArenaBuf<()> = ArenaBuf::new();
        // ZST: pushing increments len; cap is usize::MAX so no alloc.
        for _ in 0..10 {
            buf.push_within_cap(()).expect("ZST push always fits");
        }
        let tail = buf.split_off_buf(4);
        assert_eq!(buf.len(), 4);
        assert_eq!(tail.len(), 6);
        // Drop both halves cleanly.
        drop(tail);
    }

    #[test]
    fn drop_runs_live_element_destructors() {
        use core::cell::Cell;
        use core::mem::ManuallyDrop;

        struct Dropper<'c>(&'c Cell<usize>);
        impl Drop for Dropper<'_> {
            fn drop(&mut self) {
                self.0.set(self.0.get() + 1);
            }
        }

        let count = Cell::new(0);
        // Stack-backed storage that outlives `buf`. `ManuallyDrop` keeps the
        // array from dropping the elements itself, so `ArenaBuf::drop` is the
        // sole owner that runs their destructors.
        let mut storage = [
            ManuallyDrop::new(Dropper(&count)),
            ManuallyDrop::new(Dropper(&count)),
            ManuallyDrop::new(Dropper(&count)),
        ];
        let ptr = NonNull::new(storage.as_mut_ptr().cast::<Dropper<'_>>()).expect("array pointer is non-null");

        // SAFETY: `storage` holds three initialized, well-aligned `Dropper`s
        // and outlives `buf`; `ArenaBuf::drop` only drops them in place and
        // never frees the (stack-owned) backing memory.
        let buf = unsafe { ArenaBuf::from_raw_parts(ptr, 3, 3, false) };
        assert_eq!(count.get(), 0);
        drop(buf);
        assert_eq!(count.get(), 3, "ArenaBuf::drop must run the live elements' destructors");
    }
}
