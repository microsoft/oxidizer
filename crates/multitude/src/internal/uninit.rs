// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Safe "ticket" wrappers that turn raw [`InChunk`] storage into initialized
//! arena allocations.
//!
//! Each ticket type is constructed only by [`ChunkMutator`](super::ChunkMutator)
//! when it reserves storage. Consumers obtain a ticket and call the matching
//! `init*` method, which writes the value (and any drop entry) and returns a
//! safe reference. This isolates `unsafe` to a small number of methods in
//! this file; the higher layers of the crate (arena, smart pointers, vec,
//! strings) use only the safe ticket API.

use core::marker::PhantomData;
use core::mem::{self, MaybeUninit};
use core::ptr::{self, NonNull};
use core::str;

use super::drop_entry::{DropEntry, DropFn, drop_shim};
use super::in_chunk::InChunk;

/// Storage reserved for a value (or slice) that has no drop requirements.
///
/// Created by [`ChunkMutator::try_alloc_uninit`](super::ChunkMutator::try_alloc_uninit)
/// or [`try_alloc_uninit_slice`](super::ChunkMutator::try_alloc_uninit_slice).
/// Consume with [`init`](Self::init) (single value) or
/// [`init_copy_from_slice`](Self::init_copy_from_slice) (slice).
///
/// If the ticket is dropped without being initialized, the reserved bump
/// space is leaked until the owning chunk is torn down — but no unsafe
/// behavior occurs.
pub(crate) struct Uninit<'a, T: ?Sized> {
    ptr: InChunk<T>,
    _phantom: PhantomData<&'a mut T>,
}

impl<T: ?Sized> Uninit<'_, T> {
    #[inline]
    pub(super) fn new(ptr: InChunk<T>) -> Self {
        Self {
            ptr,
            _phantom: PhantomData,
        }
    }
}

impl<T: ?Sized> Uninit<'_, T> {
    /// Re-attach an `Uninit` ticket's lifetime parameter.
    ///
    /// # Safety
    ///
    /// Caller asserts that the reserved storage backing this ticket
    /// remains valid for the new lifetime `'b`. The intended use is
    /// inside [`Arena`](crate::Arena), where the chunk that hosts the
    /// slot is retained until the arena is reset or dropped — i.e.
    /// at least for the `&Arena` borrow lifetime.
    #[inline]
    pub(crate) unsafe fn rebind<'b>(self) -> Uninit<'b, T> {
        Uninit {
            ptr: self.ptr,
            _phantom: PhantomData,
        }
    }
}

impl<T: ?Sized> UninitDrop<'_, T> {
    /// Re-attach a `UninitDrop` ticket's lifetime parameter.
    ///
    /// # Safety
    ///
    /// Same contract as [`Uninit::rebind`].
    #[inline]
    pub(crate) unsafe fn rebind<'b>(self) -> UninitDrop<'b, T> {
        UninitDrop {
            value: self.value,
            drop_slot: self.drop_slot,
            _phantom: PhantomData,
        }
    }
}

impl<'a, T> Uninit<'a, T> {
    /// Writes `value` into the reserved storage and returns a mutable
    /// reference bound by the arena's lifetime.
    #[inline]
    pub(crate) fn init(self, value: T) -> &'a mut T {
        let ptr = self.init_raw(value);
        // SAFETY: `init_raw` returns a non-null pointer to an initialized
        // `T` whose storage lives for at least `'a`.
        unsafe { &mut *ptr.as_ptr() }
    }

    /// Same as [`init`](Self::init) but returns a raw pointer with no
    /// lifetime. Used by the arena layer when the resulting reference's
    /// lifetime must be tied to `&Arena` rather than to the consumed
    /// ticket's borrow scope.
    #[inline]
    pub(crate) fn init_raw(self, value: T) -> NonNull<T> {
        let raw = self.ptr.as_ptr();
        // SAFETY:
        // - `raw` is non-null and aligned for `T` (InChunk invariant).
        // - For non-ZST `T`, `raw` points at `size_of::<T>()` bytes of
        //   valid, uninitialized in-chunk storage that the mutator
        //   reserved exactly for this ticket; for ZSTs no real storage is
        //   involved.
        // - We consume `self`, so no other reference to this slot exists.
        //   `ptr::write` does not drop a prior value (we never wrote one).
        unsafe {
            ptr::write(raw, value);
            NonNull::new_unchecked(raw)
        }
    }
}

impl<'a> Uninit<'a, [u8]> {
    /// Initializes the reserved byte slice by copying `src`'s bytes and
    /// returns a `&mut str` view of the copy.
    ///
    /// The reservation must have been made with `len == src.len()`; this is
    /// enforced by [`init_copy_from_slice`](Self::init_copy_from_slice)'s
    /// debug assertion.
    #[inline]
    pub(crate) fn init_copy_from_str(self, src: &str) -> &'a mut str {
        let dst = self.init_copy_from_slice(src.as_bytes());
        // SAFETY: `dst` is a byte-for-byte copy of `src.as_bytes()`, and
        // `src: &str` carries the invariant that its bytes are valid UTF-8.
        // No other reference to `dst` exists (it was just consumed from a
        // ticket).
        unsafe { str::from_utf8_unchecked_mut(dst) }
    }
}

impl<'a, T> Uninit<'a, [T]> {
    /// Initializes the reserved slice by copying from `src` and returns a
    /// mutable reference to it.
    ///
    /// `src` must have the same length as the slice reserved at allocation
    /// time; this is enforced by debug assertion.
    #[inline]
    pub(crate) fn init_copy_from_slice(self, src: &[T]) -> &'a mut [T]
    where
        T: Copy,
    {
        let mut slice_ptr = self.init_copy_from_slice_ptr(src);
        // SAFETY: `init_copy_from_slice_ptr` returned a fully-initialized
        // slice whose lifetime is `'a`.
        unsafe { slice_ptr.as_mut() }
    }

    /// Like [`Self::init_copy_from_slice`] but returns the raw
    /// `NonNull<[T]>` with chunk-wide provenance. See
    /// [`Uninit::init_with_ptr`] for the rationale.
    #[inline]
    pub(crate) fn init_copy_from_slice_ptr(self, src: &[T]) -> NonNull<[T]>
    where
        T: Copy,
    {
        let slice_ptr = self.ptr.as_non_null();
        let len = slice_ptr.len();
        debug_assert_eq!(src.len(), len, "init_copy_from_slice: source length must match reservation");
        // SAFETY: see `init_copy_from_slice`. We deliberately avoid
        // `slice_ptr.as_mut()` so the resulting NonNull retains
        // chunk-wide provenance (no narrow `&mut [T]` retag).
        unsafe {
            let dst = slice_ptr.as_ptr().cast::<T>();
            ptr::copy_nonoverlapping(src.as_ptr(), dst, len);
        }
        slice_ptr
    }

    /// Initializes the reserved slice by cloning each element of `src` and
    /// returns a mutable reference to it. If any `T::clone` panics, all
    /// previously-cloned elements are dropped before the panic propagates.
    ///
    /// `src` must have the same length as the slice reserved at allocation
    /// time; this is enforced by debug assertion.
    #[inline]
    pub(crate) fn init_clone_from_slice(self, src: &[T]) -> &'a mut [T]
    where
        T: Clone,
    {
        debug_assert_eq!(
            src.len(),
            self.ptr.as_non_null().len(),
            "init_clone_from_slice: source length must match reservation"
        );
        self.init_with(|i| src[i].clone())
    }

    /// Initializes the reserved slice by calling `f(i)` for each index
    /// `i` in `0..len`. If `f` panics, all already-initialized elements
    /// are dropped before the panic propagates.
    #[inline]
    pub(crate) fn init_with<F>(self, f: F) -> &'a mut [T]
    where
        F: FnMut(usize) -> T,
    {
        let mut slice_ptr = self.init_with_ptr(f);
        // SAFETY: `init_with_ptr` returned a fully-initialized slice
        // whose lifetime is the `'a` of `self`.
        unsafe { slice_ptr.as_mut() }
    }

    /// Like [`Self::init_with`] but returns the raw `NonNull<[T]>` with
    /// chunk-wide provenance instead of an `&mut [T]` retag. Callers
    /// that hand the slice to a smart-pointer constructor (which then
    /// recovers the chunk header via `byte_sub`) need the chunk-wide
    /// provenance; rounding through `&mut [T]` would narrow the
    /// borrow-stack tag to the slice payload and trip strict provenance
    /// / Stacked Borrows when the header bytes are later read.
    #[inline]
    #[cfg_attr(test, mutants::skip)] // `+= → *=` on counter ⇒ infinite loop
    pub(crate) fn init_with_ptr<F>(self, mut f: F) -> NonNull<[T]>
    where
        F: FnMut(usize) -> T,
    {
        let slice_ptr = self.ptr.as_non_null();
        let len = slice_ptr.len();
        // SAFETY:
        // - Destination is `len` aligned uninitialized `T` slots (InChunk
        //   invariant); writes do not drop prior values.
        // - `InitGuard` drops any partially-initialized prefix if `f`
        //   panics, so the storage never leaks initialized `T`s.
        // - We consume `self`, so no other reference to the destination
        //   exists.
        unsafe {
            let dst = slice_ptr.as_ptr().cast::<T>();
            let mut guard = InitGuard { dst, initialized: 0 };
            while guard.initialized < len {
                dst.add(guard.initialized).write(f(guard.initialized));
                guard.initialized += 1;
            }
            mem::forget(guard);
        }
        slice_ptr
    }

    /// Initializes the reserved slice by pulling `len` values from
    /// `iter`. Panics if `iter` yields fewer elements than the
    /// reservation; in that case, already-initialized elements are
    /// dropped before the panic propagates.
    #[inline]
    pub(crate) fn init_from_iter<I>(self, iter: I) -> &'a mut [T]
    where
        I: Iterator<Item = T>,
    {
        let mut slice_ptr = self.init_from_iter_ptr(iter);
        // SAFETY: `init_from_iter_ptr` returned a fully-initialized slice
        // whose lifetime is `'a`.
        unsafe { slice_ptr.as_mut() }
    }

    /// Like [`Self::init_from_iter`] but returns the raw `NonNull<[T]>`
    /// with chunk-wide provenance. See [`Uninit::init_with_ptr`] for the
    /// rationale.
    #[inline]
    pub(crate) fn init_from_iter_ptr<I>(self, mut iter: I) -> NonNull<[T]>
    where
        I: Iterator<Item = T>,
    {
        self.init_with_ptr(|_| {
            iter.next()
                .expect("iterator yielded fewer elements than ExactSizeIterator::len() reported")
        })
    }

    /// Consume this slice ticket and return the raw start pointer plus
    /// capacity. The caller takes over responsibility for tracking
    /// which slots are initialized and for dropping the initialized
    /// prefix before the chunk is torn down.
    ///
    /// Intended for growable container backings (`Vec`, `String`)
    /// where the reservation is filled in incrementally rather than in
    /// a single `init_*` call.
    #[inline]
    pub(crate) fn into_raw_buffer(self) -> (NonNull<T>, usize) {
        let slice_ptr = self.ptr.as_non_null();
        let len = slice_ptr.len();
        (slice_ptr.cast::<T>(), len)
    }
}

/// Drop-guard used by `init_with` / `init_clone_from_slice` / `init_from_iter`
/// implementations: if the producing closure panics part-way through, drop the
/// elements written so far.
struct InitGuard<T> {
    dst: *mut T,
    initialized: usize,
}

impl<T> Drop for InitGuard<T> {
    #[inline]
    fn drop(&mut self) {
        // SAFETY: `dst[..initialized]` were each written by a successful
        // producer call above; no other references to those slots exist
        // (the parent ticket was consumed and the destination is in
        // exclusively-held chunk storage).
        unsafe {
            ptr::drop_in_place(ptr::slice_from_raw_parts_mut(self.dst, self.initialized));
        }
    }
}

/// Storage reserved for a value, paired with a pre-reserved drop entry slot.
///
/// Created by [`ChunkMutator::try_alloc_uninit_with_drop`](super::ChunkMutator::try_alloc_uninit_with_drop)
/// or [`try_alloc_uninit_slice_with_drop`](super::ChunkMutator::try_alloc_uninit_slice_with_drop).
///
/// On `init*`, the value is written into its storage and the drop entry is
/// committed (its `drop_fn` is set to a shim for `T`). If the ticket is
/// dropped without being initialized, the placeholder entry remains with no
/// drop shim — the replay loop will skip it.
pub(crate) struct UninitDrop<'a, T: ?Sized> {
    value: InChunk<T>,
    drop_slot: InChunk<DropEntry>,
    _phantom: PhantomData<&'a mut T>,
}

impl<T: ?Sized> UninitDrop<'_, T> {
    #[inline]
    pub(super) fn new(value: InChunk<T>, drop_slot: InChunk<DropEntry>) -> Self {
        Self {
            value,
            drop_slot,
            _phantom: PhantomData,
        }
    }
}

impl<'a, T> UninitDrop<'a, T> {
    /// Writes `value` into the reserved storage, commits the drop entry, and
    /// returns a mutable reference bound by the arena's lifetime.
    #[inline]
    pub(crate) fn init(self, value: T) -> &'a mut T {
        let ptr = self.init_raw(value);
        // SAFETY: `init_raw` returns a `NonNull` to an initialized `T`
        // inside chunk storage that lives for at least `'a` (by the
        // consumed ticket's invariant); the returned reference borrows that
        // storage exclusively.
        unsafe { &mut *ptr.as_ptr() }
    }
    /// Same as [`init`](Self::init) but returns a raw pointer with no
    /// lifetime. Used by the arena layer when the resulting reference's
    /// lifetime must be tied to `&Arena` rather than to the consumed
    /// ticket's borrow scope.
    #[inline]
    pub(crate) fn init_raw(self, value: T) -> NonNull<T> {
        let raw = self.value.as_ptr();
        let entry = self.drop_slot.as_ptr();
        // SAFETY:
        // - `raw` and `entry` are non-null, aligned, and address in-chunk
        //   storage exclusively reserved for this ticket (InChunk invariants
        //   + consumed-on-init).
        // - The value slot is uninitialized; `ptr::write` does not drop a
        //   prior value.
        // - `entry` currently holds a placeholder `DropEntry` (written at
        //   reservation time), so dereferencing it as `&mut DropEntry` is
        //   sound and does not alias any other reference.
        unsafe {
            ptr::write(raw, value);
            (*entry).commit_drop_fn(drop_shim::<T> as DropFn);
            NonNull::new_unchecked(raw)
        }
    }

    /// Writes a (possibly uninitialized) `MaybeUninit<T>` into the value
    /// slot and returns a pointer to it, leaving the pre-reserved drop entry
    /// as an **uncommitted** placeholder.
    ///
    /// Used by the uninit-`Arc` allocation path: the entry is committed
    /// later by [`Arc::<MaybeUninit<T>>::assume_init`](crate::Arc) once the
    /// value is initialized. If the resulting handle is dropped without
    /// `assume_init`, the placeholder stays `None` and the replay loop skips
    /// it, so no destructor runs on uninitialized memory.
    #[inline]
    pub(crate) fn into_uninit_placeholder(self, value: MaybeUninit<T>) -> NonNull<MaybeUninit<T>> {
        let raw = self.value.as_ptr().cast::<MaybeUninit<T>>();
        // SAFETY: `raw` is non-null, aligned for `T` (identical to
        // `MaybeUninit<T>`), and exclusively owned by this consumed ticket;
        // the slot is uninitialized so `write` drops nothing. The drop slot
        // is intentionally left as the placeholder written at reservation.
        unsafe {
            ptr::write(raw, value);
            NonNull::new_unchecked(raw)
        }
    }
}

impl<'a, T> UninitDrop<'a, [T]> {
    /// Initializes the reserved slice by cloning each element of `src`,
    /// commits the drop entry, and returns a mutable reference bound by
    /// the arena's lifetime.
    ///
    /// If any `T::clone` panics, all previously-cloned elements are
    /// dropped before the panic propagates; the drop entry is *not*
    /// committed (the chunk's drop-replay loop will skip the placeholder),
    /// so partially-initialized memory cannot be re-dropped at arena
    /// teardown.
    #[inline]
    pub(crate) fn init_clone_from_slice(self, src: &[T]) -> &'a mut [T]
    where
        T: Clone,
    {
        debug_assert_eq!(
            src.len(),
            self.value.as_non_null().len(),
            "init_clone_from_slice: source length must match reservation"
        );
        self.init_with(|i| src[i].clone())
    }

    /// Initializes the reserved slice by calling `f(i)` for each index
    /// `i` in `0..len`, then commits the drop entry on success. If `f`
    /// panics, already-initialized elements are dropped and the drop
    /// entry is *not* committed (the chunk's drop-replay loop skips the
    /// placeholder).
    #[inline]
    pub(crate) fn init_with<F>(self, f: F) -> &'a mut [T]
    where
        F: FnMut(usize) -> T,
    {
        let mut slice_ptr = self.init_with_ptr(f);
        // SAFETY: `init_with_ptr` returned a fully-initialized slice
        // whose lifetime is the `'a` of `self`.
        unsafe { slice_ptr.as_mut() }
    }

    /// Like [`Self::init_with`] but returns the raw `NonNull<[T]>` with
    /// chunk-wide provenance. See [`Uninit::init_with_ptr`] for the
    /// rationale.
    #[inline]
    #[cfg_attr(test, mutants::skip)] // counter mutation += → *= ⇒ infinite loop
    pub(crate) fn init_with_ptr<F>(self, mut f: F) -> NonNull<[T]>
    where
        F: FnMut(usize) -> T,
    {
        let slice_ptr = self.value.as_non_null();
        let len = slice_ptr.len();
        let entry = self.drop_slot.as_ptr();
        // SAFETY: see `Uninit::init_with_ptr` for the init-loop invariants
        // and `UninitDrop::init` for the drop-slot invariants. The drop
        // entry is committed only after all initializations succeed.
        unsafe {
            let dst = slice_ptr.as_ptr().cast::<T>();
            let mut guard = InitGuard { dst, initialized: 0 };
            while guard.initialized < len {
                dst.add(guard.initialized).write(f(guard.initialized));
                guard.initialized += 1;
            }
            mem::forget(guard);
            (*entry).commit_drop_fn(drop_shim::<T> as DropFn);
        }
        slice_ptr
    }

    /// Initializes the reserved slice by pulling `len` values from
    /// `iter` and commits the drop entry on success. Panics if `iter`
    /// yields fewer elements than the reservation; in that case,
    /// already-initialized elements are dropped and the drop entry is
    /// not committed.
    #[inline]
    pub(crate) fn init_from_iter<I>(self, mut iter: I) -> &'a mut [T]
    where
        I: Iterator<Item = T>,
    {
        self.init_with(|_| {
            iter.next()
                .expect("iterator yielded fewer elements than ExactSizeIterator::len() reported")
        })
    }

    /// Slice analogue of [`UninitDrop::into_uninit_placeholder`]: optionally
    /// zero-fills the reserved elements and returns the buffer as
    /// `MaybeUninit<T>`s, leaving the pre-reserved drop entry **uncommitted**.
    ///
    /// The uninit-slice-`Arc` path commits the entry later via
    /// [`Arc::<[MaybeUninit<T>]>::assume_init`](crate::Arc).
    #[inline]
    pub(crate) fn into_uninit_slice_placeholder(self, zeroed: bool) -> NonNull<[MaybeUninit<T>]> {
        let slice_ptr = self.value.as_non_null();
        let len = slice_ptr.len();
        let base = slice_ptr.cast::<MaybeUninit<T>>();
        if zeroed {
            // SAFETY: `base` addresses `len` exclusively-owned `MaybeUninit<T>`
            // slots inside chunk storage reserved for this consumed ticket;
            // zeroing their bytes leaves valid `MaybeUninit<T>` values.
            unsafe {
                ptr::write_bytes(base.as_ptr().cast::<u8>(), 0, len.saturating_mul(mem::size_of::<T>()));
            }
        }
        NonNull::slice_from_raw_parts(base, len)
    }
}
