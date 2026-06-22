// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![allow(
    clippy::type_repetition_in_bounds,
    clippy::cast_sign_loss,
    reason = "trait-impl `where` clauses are kept uniform across all forwarding impls; numeric casts are bounded by upstream `usize` checks documented at call sites"
)]

use core::borrow::BorrowMut;
use core::iter::FusedIterator;
use core::marker::PhantomData;
use core::mem::{self, MaybeUninit};
use core::ops::DerefMut;
use core::pin::Pin;
use core::ptr::{self, NonNull};

use allocator_api2::alloc::{Allocator, Global};
use ptr_meta::Pointee;

use crate::internal::chunk_ref::ChunkRef;
use crate::thin_smart_ptr_common::impl_thin_smart_ptr_common;

/// An owned, mutable smart pointer to a `T` stored in an
/// [`Arena`](crate::Arena).
///
/// Created via [`Arena::alloc_box`](crate::Arena::alloc_box).
///
/// Unlike [`Arc`](crate::Arc):
///
/// - Provides `&mut T` through `DerefMut` (exclusive ownership).
/// - **Not** [`Clone`] — single owner.
///
/// Like [`Arc`](crate::Arc), `Box` keeps its containing chunk alive by
/// holding a +1 refcount, so it can outlive the arena it came from and
/// survives [`Arena::reset`](crate::Arena::reset), and it runs `T`'s
/// destructor eagerly — never deferred to chunk teardown. As the sole
/// owner, `Box` drops `T` when the `Box` itself is dropped, whereas
/// `Arc` drops `T` when its last clone is dropped.
///
/// # `Send` and `Sync`
///
/// `Box<T, A>` is [`Send`] when `T: Send` and `A: Send + Sync`, and
/// [`Sync`] when `T: Sync` and `A: Sync`. The backing storage lives in a
/// shared chunk whose refcount is atomic; a last-reference `Drop` on the
/// receiving thread tears that chunk down through its
/// `Weak<ChunkProvider<A>>` (which touches the shared provider/allocator),
/// so `Send` requires `A: Sync` too — exactly as `Arc<T, A>` does, rather
/// than `std::boxed::Box<T, A>`'s `A: Send` (whose `A` is uniquely owned).
///
/// # Pinning
///
/// `Box` implements [`Unpin`] unconditionally (like `std::Box`).
/// Pinning a `Box` is sound: because `Box` holds a +1 refcount on its
/// chunk, the backing memory cannot be freed or reused while the
/// `Box` exists. If a pinned `Box` is leaked via [`core::mem::forget`],
/// the refcount is never decremented and the chunk's storage persists
/// for the lifetime of the process — satisfying [`Pin`](core::pin::Pin)'s
/// drop guarantee (the pinned value's memory is never reclaimed).
///
/// # Example
///
/// ```
/// use multitude::Arena;
///
/// let arena = Arena::new();
/// let mut b = arena.alloc_box(vec![1, 2, 3]);
/// b.push(4);
/// assert_eq!(*b, vec![1, 2, 3, 4]);
/// ```
pub struct Box<T: ?Sized + Pointee, A: Allocator + Clone = Global> {
    /// **Thin** pointer to the first byte of the contained value, which
    /// lives in a 64K-aligned [`SharedChunk`]'s payload. The chunk
    /// header is recovered by masking, and `T`'s pointer metadata (if
    /// any) is stored in the `size_of::<T::Metadata>()` bytes
    /// immediately preceding the payload (read with
    /// [`core::ptr::read_unaligned`]). This makes `Box<T>` 8 bytes
    /// uniformly, even for DST `T`.
    ptr: NonNull<u8>,
    /// Marker for variance and dropck. The raw-pointer wrapping keeps
    /// auto-derivation conservative; the desired `Send`/`Sync` are
    /// re-introduced via the explicit `unsafe impl`s below so the
    /// bounds match `std::boxed::Box<T, A>`.
    _phantom: PhantomData<(*const T, A)>,
}

// SAFETY: `Box<T, A>` owns its `T` uniquely (no aliasing), and the
// storage refcount is managed by the chunk's atomic counter, so the
// `dec_ref` performed in `Drop` is thread-safe regardless of which
// thread allocated the `Box`. Sending the `Box` to another thread is
// sound when `T: Send` (the value moves).
//
// Unlike `std::boxed::Box<T, A>`, which uniquely owns `A`, this `Box`
// does NOT own its allocator: `A` lives in the shared, refcounted
// chunk header alongside a `Weak<ChunkProvider<A>>`, and a *last*-ref
// `Drop` on the receiving thread tears the chunk down through that
// shared provider (`teardown_and_release` -> `Weak::upgrade` ->
// `ChunkProvider::release_shared`, which may run `A::deallocate`). That
// foreign-thread access to the shared provider requires `A: Sync` (and
// `Weak<ChunkProvider<A>>: Send` needs `A: Send + Sync`), exactly as
// `Arc<T, A>` requires. Hence the `Send` bound is `A: Send + Sync`, not
// `std`'s `A: Send`. The `Pointee` bound is implicit (already on the
// `Box` struct).
unsafe impl<T: ?Sized + Pointee + Send, A: Allocator + Clone + Send + Sync> Send for Box<T, A> {}
// SAFETY: see the `Send` impl above for the cross-thread invariants.
// Sharing `&Box<T, A>` across threads exposes only `&T` (`Deref` is
// `&self -> &T`); `DerefMut` requires `&mut self` and is serialized
// by the borrow checker. So `Sync` follows `T: Sync`, with `A: Sync`
// mirrored from `std::boxed::Box<T, A>`.
unsafe impl<T: ?Sized + Pointee + Sync, A: Allocator + Clone + Sync> Sync for Box<T, A> {}

impl<T: ?Sized + Pointee, A: Allocator + Clone> Box<T, A> {
    /// Builds a `Box` from a thin payload pointer.
    ///
    /// See [`crate::Arc::from_raw`] for the metadata-recovery contract.
    ///
    /// # Safety
    ///
    /// - `thin` must reference the payload of a fully-initialized `T`
    ///   whose storage was bump-allocated from a [`SharedChunk<A>`].
    ///   For DST `T` the chunk prefix must carry the matching
    ///   `T::Metadata`.
    /// - The caller must have just acquired a +1 refcount on that
    ///   chunk in the new `Box`'s name. The returned `Box` takes
    ///   ownership of that +1 and releases it in [`Drop`].
    /// - `thin` must lie within the first `CHUNK_ALIGN` bytes of the
    ///   chunk's allocation so the header-from-mask helper recovers
    ///   the chunk address correctly.
    #[inline]
    pub(crate) unsafe fn from_raw(thin: NonNull<u8>) -> Self {
        Self {
            ptr: thin,
            _phantom: PhantomData,
        }
    }

    /// Returns the thin chunk pointer (see [`crate::Arc::thin_ptr`]).
    #[inline]
    pub(crate) fn thin_ptr(&self) -> NonNull<u8> {
        self.ptr
    }

    /// Returns a raw mutable pointer to the value (fat if `T: ?Sized` is a DST).
    #[allow(
        clippy::needless_pass_by_ref_mut,
        reason = "associated-fn convention (like alloc::rc::Rc::as_ptr); &mut self conveys exclusive access"
    )]
    #[must_use]
    #[inline]
    pub fn as_mut_ptr(this: &mut Self) -> *mut T {
        this.as_fat_ptr().as_ptr()
    }
}

impl_thin_smart_ptr_common!(Box);

// No `leak`: dropping the refcount risks UAF; keeping it leaks the chunk.

impl<T: ?Sized + Pointee, A: Allocator + Clone> Drop for Box<T, A> {
    #[inline]
    fn drop(&mut self) {
        // Adopt the chunk's +1 before running `T::drop`, so a panic in
        // `T::drop` still releases the refcount via `ChunkRef`'s own `Drop`
        // during unwinding (the in-chunk slot leaks, per documented panic
        // semantics).
        //
        // SAFETY: `ptr` is hosted in a 64K-aligned `SharedChunk` we hold a +1
        // strong reference on; `ChunkRef::from_value_ptr` adopts that +1 and
        // releases it on drop. `T::drop` then runs in place (elided when
        // `needs_drop::<T>()` is false).
        unsafe {
            let _ref: ChunkRef<A> = ChunkRef::from_value_ptr(self.ptr);
            let fat = self.as_fat_ptr();
            ptr::drop_in_place(fat.as_ptr());
        }
    }
}

impl<T, A: Allocator + Clone> Box<MaybeUninit<T>, A> {
    /// Convert an [`Box<MaybeUninit<T>, A>`] whose value has been
    /// fully initialized into an [`Box<T, A>`]. O(1) — no copy,
    /// no allocation.
    ///
    /// # Safety
    ///
    /// The `MaybeUninit<T>` must contain a fully-initialized, valid `T`.
    #[must_use]
    #[inline]
    pub unsafe fn assume_init(self) -> Box<T, A> {
        let thin = self.ptr;
        mem::forget(self);
        // SAFETY: see scalar `Arc::<MaybeUninit<T>>::assume_init` —
        // sized `T` shares the same chunk layout (no prefix) as
        // `MaybeUninit<T>`.
        unsafe { Box::from_raw(thin) }
    }

    /// Convert a pinned `Pin<Box<MaybeUninit<T>, A>>` whose value has
    /// been fully initialized into a `Pin<Box<T, A>>`. O(1).
    ///
    /// The pin is preserved across the cast: the value's address is
    /// the same `Box` allocation's address; nothing moves.
    ///
    /// # Safety
    ///
    /// The `MaybeUninit<T>` must contain a fully-initialized, valid `T`.
    #[must_use]
    #[inline]
    pub unsafe fn assume_init_pin(this: Pin<Self>) -> Pin<Box<T, A>>
    where
        A: 'static,
    {
        // SAFETY: see `Pin::map_unchecked` + `Self::assume_init`; the
        // value's address is unchanged across this cast, and the
        // caller asserts the contents are a valid `T`.
        unsafe {
            let inner: Self = Pin::into_inner_unchecked(this);
            Box::into_pin(inner.assume_init())
        }
    }
}

impl<T, A: Allocator + Clone> Box<[MaybeUninit<T>], A> {
    /// Convert an [`Box<[MaybeUninit<T>], A>`](crate::Box) whose elements have
    /// all been fully initialized into an [`Box<[T], A>`](crate::Box). O(1) —
    /// no copy, no allocation.
    ///
    /// # Safety
    ///
    /// Every element of the slice must contain a fully-initialized,
    /// valid `T`.
    #[must_use]
    #[inline]
    pub unsafe fn assume_init(self) -> Box<[T], A> {
        let thin = self.ptr;
        mem::forget(self);
        // SAFETY: see scalar `assume_init`; `Box<[MaybeUninit<T>]>` and
        // `Box<[T]>` share the same chunk prefix layout (slice length
        // as `usize`), so the length stored there already matches the
        // new fat pointer's metadata.
        unsafe { Box::from_raw(thin) }
    }

    /// Pinned-slice variant of [`Self::assume_init_pin`]. The slice's
    /// element addresses don't change across the cast.
    ///
    /// # Safety
    ///
    /// Every element must contain a fully-initialized, valid `T`.
    #[must_use]
    #[inline]
    pub unsafe fn assume_init_pin_slice(this: Pin<Self>) -> Pin<Box<[T], A>>
    where
        A: 'static,
    {
        // SAFETY: see `Pin::map_unchecked` + `Self::assume_init`; the
        // value's address is unchanged across this cast, and the
        // caller asserts every element is a valid `T`.
        unsafe {
            let inner: Self = Pin::into_inner_unchecked(this);
            Box::into_pin(inner.assume_init())
        }
    }
}

impl<T: ?Sized + Pointee, A: Allocator + Clone> DerefMut for Box<T, A> {
    #[inline]
    fn deref_mut(&mut self) -> &mut T {
        // SAFETY: see `Deref`; `&mut self` confirms exclusive access.
        unsafe { self.as_fat_ptr().as_mut() }
    }
}

impl<T: ?Sized + Pointee, A: Allocator + Clone> AsMut<T> for Box<T, A> {
    #[inline]
    fn as_mut(&mut self) -> &mut T {
        self
    }
}

impl<T: ?Sized + Pointee, A: Allocator + Clone> BorrowMut<T> for Box<T, A> {
    #[inline]
    fn borrow_mut(&mut self) -> &mut T {
        self
    }
}

impl<I: Iterator + ?Sized + Pointee, A: Allocator + Clone> Iterator for Box<I, A> {
    type Item = I::Item;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        (**self).next()
    }

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        (**self).size_hint()
    }

    #[inline]
    fn nth(&mut self, n: usize) -> Option<Self::Item> {
        (**self).nth(n)
    }
}

impl<I: DoubleEndedIterator + ?Sized + Pointee, A: Allocator + Clone> DoubleEndedIterator for Box<I, A> {
    #[inline]
    fn next_back(&mut self) -> Option<Self::Item> {
        (**self).next_back()
    }

    #[inline]
    fn nth_back(&mut self, n: usize) -> Option<Self::Item> {
        (**self).nth_back(n)
    }
}

impl<I: ExactSizeIterator + ?Sized + Pointee, A: Allocator + Clone> ExactSizeIterator for Box<I, A> {
    #[inline]
    fn len(&self) -> usize {
        (**self).len()
    }
}

impl<I: FusedIterator + ?Sized + Pointee, A: Allocator + Clone> FusedIterator for Box<I, A> {}

impl<'a, T, A: Allocator + Clone> From<crate::vec::Vec<'a, T, A>> for Box<[T], A> {
    /// Freeze a [`Vec`](crate::vec::Vec) into an immutable
    /// [`Box<[T], A>`](crate::Box). Mirrors `std`'s `From<Vec<T>> for Box<[T]>`.
    #[inline]
    fn from(v: crate::vec::Vec<'a, T, A>) -> Self {
        v.into_boxed_slice()
    }
}
