// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![allow(
    clippy::type_repetition_in_bounds,
    reason = "trait-impl `where` clauses are kept uniform across all forwarding impls"
)]

use core::marker::PhantomData;
use core::mem::{self, MaybeUninit};
use core::pin::Pin;
use core::ptr::{self, NonNull};

use allocator_api2::alloc::{Allocator, Global};
use ptr_meta::Pointee;

use crate::internal::chunk::Chunk;
use crate::internal::chunk_ref::ChunkRef;
use crate::internal::drop_entry::{self, DropFn};
use crate::internal::shared_chunk::SharedChunk;
use crate::internal::thin_dst;
use crate::thin_smart_ptr_common::impl_thin_smart_ptr_common;
use crate::vec::Vec;

/// A thread-safe reference-counted smart pointer to a `T` stored in an [`Arena`](crate::Arena).
///
/// Safe to share across threads when `T: Send + Sync`.
///
/// Created via [`Arena::alloc_arc`](crate::Arena::alloc_arc). Cloning is
/// **O(1)** and uses a single Relaxed atomic increment (matching
/// `std::sync::Arc`). Dropping a clone is one Release decrement plus,
/// on the final dec to zero, an Acquire fence before chunk teardown.
///
/// `Arc` keeps its containing chunk alive by holding a +1 refcount on
/// it, so the smart pointer can outlive the arena it came from and
/// survives [`Arena::reset`](crate::Arena::reset). For `T: Drop`, a
/// drop entry is registered at allocation time and `T::drop` runs at
/// chunk teardown (when the chunk's last reference is released); for
/// `T: !Drop` (the common case for strings, numbers, slices, etc.),
/// no drop entry is reserved and the only per-allocation cost beyond
/// the value itself is the chunk's atomic refcount.
///
/// # Pinning
///
/// `Arc` implements [`Unpin`] unconditionally (like `std::sync::Arc`).
///
/// # Example
///
/// ```
/// use std::thread;
///
/// use multitude::Arena;
///
/// let arena = Arena::new();
/// let a = arena.alloc_arc(42_u32);
/// let b = a.clone();
/// let h = thread::spawn(move || *b);
/// assert_eq!(*a, h.join().unwrap());
/// ```
pub struct Arc<T: ?Sized + Pointee, A: Allocator + Clone = Global> {
    /// **Thin** pointer to the first byte of the contained value, which
    /// lives in a 64K-aligned [`SharedChunk`](crate::internal::shared_chunk::SharedChunk)'s
    /// payload. The chunk header is recovered by masking, and `T`'s
    /// pointer metadata (if any — `()` for `T: Sized`, `usize` for
    /// slice DSTs / `str`, vtable for trait objects) is stored in the
    /// `size_of::<T::Metadata>()` bytes immediately preceding the
    /// payload (read with [`core::ptr::read_unaligned`]).
    ///
    /// This makes `Arc<T>` 8 bytes uniformly, even for DST `T`.
    ptr: NonNull<u8>,
    /// Variance + dropck marker. Send/Sync are gated by explicit
    /// unsafe impls below.
    _phantom: PhantomData<(*const T, A)>,
}

impl<T: ?Sized + Pointee, A: Allocator + Clone> Arc<T, A> {
    /// Builds an `Arc` from a thin payload pointer.
    ///
    /// For DST `T`, the metadata is recovered on demand from the chunk
    /// prefix at `thin - size_of::<T::Metadata>()` via `as_fat_ptr`; the
    /// caller must have already written it there at allocation time.
    /// For `T: Sized`, the prefix is zero-sized and no metadata is
    /// stored.
    ///
    /// # Safety
    ///
    /// - `thin` must reference the payload of a fully-initialized `T`
    ///   whose storage was bump-allocated from a [`SharedChunk<A>`] via
    ///   the thin-DST allocator path. For DST `T` the chunk prefix
    ///   must carry the matching `T::Metadata`. For `T: Drop`, a drop
    ///   entry must already be registered so the destructor runs at
    ///   chunk teardown.
    /// - The caller must have just acquired a +1 refcount on that chunk
    ///   in the new `Arc`'s name; the returned `Arc` takes ownership of
    ///   that +1 and releases it in [`Drop`].
    /// - `thin` must lie within the first `CHUNK_ALIGN` bytes of the
    ///   chunk so the header-from-mask helper recovers the chunk
    ///   address correctly.
    #[inline]
    pub(crate) unsafe fn from_raw(thin: NonNull<u8>) -> Self {
        Self {
            ptr: thin,
            _phantom: PhantomData,
        }
    }

    /// Returns the thin chunk pointer — the byte address of the
    /// value's payload inside its hosting chunk. Carries chunk-wide
    /// provenance (no `&T` narrowing). Used by string-flavored
    /// conversions in `strings/str_impls.rs` to retag between
    /// `Arc<str>` and `Arc<[u8]>` without losing the chunk-recovery
    /// borrow-stack tag the smart pointer's `Drop` walks back through.
    #[inline]
    pub(crate) fn thin_ptr(&self) -> NonNull<u8> {
        self.ptr
    }

    /// True iff both handles point at the same address.
    #[inline]
    #[must_use]
    pub fn ptr_eq(a: &Self, b: &Self) -> bool {
        ptr::addr_eq(a.ptr.as_ptr(), b.ptr.as_ptr())
    }
}

impl_thin_smart_ptr_common!(Arc);

impl<T, A: Allocator + Clone> Arc<MaybeUninit<T>, A> {
    /// Convert a handle to `MaybeUninit<T>` whose value is now
    /// initialized into a handle to `T`. O(1) — no copy or alloc.
    ///
    /// # Safety
    ///
    /// The `MaybeUninit<T>` must contain a fully-initialized, valid
    /// `T`. The allocation must come from
    /// [`Arena::alloc_uninit_arc`](crate::Arena::alloc_uninit_arc) or
    /// [`Arena::alloc_zeroed_arc`](crate::Arena::alloc_zeroed_arc) so a
    /// drop entry was reserved up front;
    /// `Arena::alloc_arc(MaybeUninit::new(...))` does not reserve one
    /// and panics here for `T: Drop`.
    ///
    /// # Panics
    ///
    /// Panics for `T: Drop` when no drop entry is found in the chunk
    /// — see the safety contract above.
    #[inline]
    #[must_use]
    pub unsafe fn assume_init(self) -> Arc<T, A> {
        if const { mem::needs_drop::<T>() } {
            // SAFETY: `self.ptr` references a live value inside a
            // `SharedChunk<A>` this `Arc` holds a +1 on; `alloc_uninit_arc`
            // reserved a placeholder drop entry for it. Commit the real shim
            // so `T::drop` runs at chunk teardown.
            unsafe {
                commit_uninit_drop_entry::<A>(self.ptr, 1, drop_entry::drop_shim::<T>, false);
            }
        }
        let thin = self.ptr;
        mem::forget(self);
        // SAFETY: `thin` carries the +1 the consumed handle held; the value is
        // now a valid `T` per the caller's contract. `Arc<MaybeUninit<T>>` and
        // `Arc<T>` for sized `T` share the same chunk layout (no metadata
        // prefix), so no prefix rewrite is needed.
        unsafe { Arc::from_raw(thin) }
    }

    /// Pinned mirror of [`Self::assume_init`]. The pin is preserved
    /// across the cast because the value's address does not change.
    ///
    /// # Safety
    ///
    /// Same contract as [`Self::assume_init`].
    #[must_use]
    #[inline]
    pub unsafe fn assume_init_pin(this: Pin<Self>) -> Pin<Arc<T, A>>
    where
        A: 'static,
    {
        // SAFETY: see `Pin::map_unchecked` + `Self::assume_init`; the
        // value's address is unchanged across this cast, and the
        // caller asserts the contents are a valid `T`.
        unsafe {
            let inner: Self = Pin::into_inner_unchecked(this);
            Arc::into_pin(inner.assume_init())
        }
    }
}

impl<T, A: Allocator + Clone> Arc<[MaybeUninit<T>], A> {
    /// Convert an initialized `Arc<[MaybeUninit<T>]>` into an `Arc<[T]>`.
    ///
    /// O(1) — reinterprets the existing handle in place.
    ///
    /// # Safety
    ///
    /// Every element of the slice must contain a fully-initialized,
    /// valid `T`. The allocation must come from
    /// [`Arena::alloc_uninit_slice_arc`](crate::Arena::alloc_uninit_slice_arc)
    /// or
    /// [`Arena::alloc_zeroed_slice_arc`](crate::Arena::alloc_zeroed_slice_arc).
    ///
    /// # Panics
    ///
    /// Panics for `T: Drop` when no drop entry is found in the chunk.
    #[inline]
    #[must_use]
    pub unsafe fn assume_init(self) -> Arc<[T], A> {
        // SAFETY: `Arc<[MaybeUninit<T>]>` and `Arc<[T]>` share an
        // identical chunk prefix layout (the slice length, written as
        // `usize` by the allocator); read the length from the prefix
        // directly rather than relying on the (now-thin) `self.ptr`.
        let len: usize = unsafe { thin_dst::read_metadata::<[T]>(self.ptr) };
        if const { mem::needs_drop::<T>() } {
            // SAFETY: see the scalar `assume_init`; the placeholder slice
            // drop entry reserved by `alloc_uninit_slice_arc` is committed to
            // `drop_shim::<T>` so all `len` elements drop at chunk teardown.
            unsafe {
                commit_uninit_drop_entry::<A>(self.ptr, len, drop_entry::drop_shim::<T>, true);
            }
        }
        let thin = self.ptr;
        mem::forget(self);
        // SAFETY: `thin` carries the +1 the consumed handle held; every
        // element is now a valid `T` per the caller's contract.
        // `Arc<[MaybeUninit<T>]>` and `Arc<[T]>` share the same chunk
        // prefix layout, so the length already stored there matches the
        // new fat pointer's metadata.
        unsafe { Arc::from_raw(thin) }
    }

    /// Pinned mirror of [`Self::assume_init`] for slices.
    ///
    /// # Safety
    ///
    /// Same contract as [`Self::assume_init`].
    #[must_use]
    #[inline]
    pub unsafe fn assume_init_pin_slice(this: Pin<Self>) -> Pin<Arc<[T], A>>
    where
        A: 'static,
    {
        // SAFETY: see `Pin::map_unchecked` + `Self::assume_init`; the
        // value's address is unchanged across this cast, and the
        // caller asserts every element is a valid `T`.
        unsafe {
            let inner: Self = Pin::into_inner_unchecked(this);
            Arc::into_pin(inner.assume_init())
        }
    }
}

/// Locates the placeholder [`DropEntry`](crate::internal::drop_entry) that
/// `Arena::alloc_uninit_arc` / `alloc_uninit_slice_arc` reserved for the
/// value at `value` and commits `drop_fn` into it, so the value's destructor
/// runs when the hosting chunk is torn down.
///
/// `len` is `1` for a scalar value or the element count for a slice.
/// `is_slice` only selects the panic message.
///
/// # Safety
///
/// - `value` must point at a value reserved via the uninit-`Arc` path, living
///   in the first `CHUNK_ALIGN` bytes of a live `SharedChunk<A>` on which the
///   caller holds a strong reference.
/// - `assume_init` must be called at most once per allocation (the placeholder
///   commit is a non-atomic write; concurrent commits on cloned handles are
///   not supported).
#[inline]
unsafe fn commit_uninit_drop_entry<A: Allocator + Clone>(value: NonNull<u8>, len: usize, drop_fn: DropFn, is_slice: bool) {
    let header = SharedChunk::<A>::header_from_value_ptr(value);
    // SAFETY: `header` has full chunk provenance via `with_addr`;
    // reconstruct the fat DST pointer for typed field access.
    let chunk = unsafe { NonNull::new_unchecked(SharedChunk::<A>::header_to_fat(header.as_ptr())) };
    // SAFETY: `chunk` is a live `SharedChunk<A>` (caller holds a +1).
    let chunk_ref = unsafe { chunk.as_ref() };
    // SAFETY: `chunk` is live; `payload_ptr` returns its payload start.
    let payload = unsafe { SharedChunk::<A>::payload_ptr(chunk) }.as_ptr();
    let payload_len = chunk_ref.capacity();
    let value_offset = (value.as_ptr() as usize) - (payload as usize);
    // Acquire pairs with the owner thread's Release publish of the count in
    // `ChunkMutator::publish_drop_count`, so the placeholder slot's bytes are
    // visible to this (possibly different) thread before we read/commit it.
    let count = chunk_ref.drop_entry_count_acquire();
    // SAFETY: `payload`, `payload_len`, and `count` describe the live chunk's
    // drop region; we hold a +1 and the contract forbids concurrent commits.
    let committed = unsafe { drop_entry::commit_placeholder_drop_fn(payload, payload_len, count, value_offset, len, drop_fn) };
    assert!(
        committed,
        "{}",
        if is_slice {
            "Arc::<[MaybeUninit<T>]>::assume_init: no drop entry reserved for this allocation. \
             Use `Arena::alloc_uninit_slice_arc::<T>()` / `alloc_zeroed_slice_arc`; allocating \
             a `MaybeUninit<T>` slice via the ordinary slice-Arc helpers does not reserve one \
             and would silently leak each `T::drop`."
        } else {
            "Arc::<MaybeUninit<T>>::assume_init: no drop entry reserved for this allocation. \
             Use `Arena::alloc_uninit_arc::<T>()` / `alloc_zeroed_arc`; \
             `Arena::alloc_arc(MaybeUninit::new(...))` does not reserve an entry and would \
             silently leak `T::drop`."
        }
    );
}

impl<T: ?Sized + Pointee, A: Allocator + Clone> Clone for Arc<T, A> {
    #[inline]
    fn clone(&self) -> Self {
        // SAFETY: `self` owns a live +1 on its chunk so the chunk is
        // alive; `clone_from_value_ptr` mints a fresh +1 via an
        // atomic bump and returns a `ChunkRef` that owns it. We
        // `forget` that `ChunkRef`, handing the +1 to the new `Arc`.
        let chunk_ref = unsafe { ChunkRef::<A>::clone_from_value_ptr(self.ptr) };
        let _ = chunk_ref.forget();
        Self {
            ptr: self.ptr,
            _phantom: PhantomData,
        }
    }
}

impl<T: ?Sized + Pointee, A: Allocator + Clone> Drop for Arc<T, A> {
    #[inline]
    fn drop(&mut self) {
        // SAFETY: `ptr` is hosted in a 64K-aligned SharedChunk we
        // hold a +1 strong reference on. `ChunkRef::from_value_ptr`
        // adopts that +1 and releases it on its own drop. We do not
        // invoke `T::drop` here — for `T: Drop`, a drop entry was
        // registered at allocation time so the chunk's teardown runs
        // `T::drop` when the last reference releases the chunk; for
        // `T: !Drop` no destructor is needed.
        unsafe {
            let _ref: ChunkRef<A> = ChunkRef::from_value_ptr(self.ptr);
        }
    }
}

// SAFETY: same cross-thread invariants as `std::sync::Arc`; the backing
// chunk refcount is atomic and sharing is gated on `T` and `A`.
unsafe impl<T: ?Sized + Pointee + Sync + Send, A: Allocator + Clone + Send + Sync> Send for Arc<T, A> {}
// SAFETY: same invariants as the `Send` impl.
unsafe impl<T: ?Sized + Pointee + Sync + Send, A: Allocator + Clone + Send + Sync> Sync for Arc<T, A> {}

impl<'a, T, A: Allocator + Clone> From<Vec<'a, T, A>> for Arc<[T], A>
where
    T: Send + Sync,
    A: Send + Sync,
{
    /// Freeze a [`Vec`](crate::vec::Vec) into an immutable
    /// [`Arc<[T], A>`](crate::Arc). Mirrors `std`'s `From<Vec<T>> for Arc<[T]>`.
    #[inline]
    fn from(v: Vec<'a, T, A>) -> Self {
        v.freeze_into_arc()
    }
}
