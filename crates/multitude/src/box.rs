// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

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
use crate::vec::Vec;

/// An owned, mutable smart pointer to an arena-backed `T`.
///
/// Created via [`Arena::alloc_box`](crate::Arena::alloc_box).
///
/// Unlike [`Arc`](crate::Arc):
///
/// - Provides `&mut T` through `DerefMut` (exclusive ownership).
/// - **Not** [`Clone`] — single owner.
///
/// Like [`Arc`](crate::Arc), `Box` can outlive the arena it came from and
/// survives [`Arena::reset`](crate::Arena::reset), and it runs `T`'s
/// destructor eagerly. As the sole owner, `Box` drops `T` when the `Box`
/// itself is dropped, whereas `Arc` drops `T` when its last clone is dropped.
///
/// # `Send` and `Sync`
///
/// `Box<T, A>` is [`Send`] when `T: Send` and `A: Send + Sync`, and
/// [`Sync`] when `T: Sync` and `A: Sync` — the same bounds as `Arc<T, A>`
/// (rather than `std::boxed::Box<T, A>`'s `A: Send`), because the backing
/// storage is shared with the arena's chunk machinery.
///
/// # Pinning
///
/// `Box` implements [`Unpin`] unconditionally (like `std::Box`).
/// Pinning a `Box` is sound: the value stays at a fixed address for as long as
/// the `Box` (or a pinned, leaked `Box`) exists, satisfying
/// [`Pin`](core::pin::Pin)'s drop guarantee.
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
    /// lives in a 64K-aligned [`Chunk`]'s payload. The chunk
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
// does NOT own its allocator: `A` lives in the refcounted chunk header
// alongside a `Weak<ChunkProvider<A>>`, and a *last*-ref `Drop` on the
// receiving thread tears the chunk down through that provider
// (`teardown_and_release` -> `Weak::upgrade` -> `ChunkProvider::release`,
// which may run `A::deallocate`). That foreign-thread access to the
// provider requires `A: Sync` (and `Weak<ChunkProvider<A>>: Send` needs
// `A: Send + Sync`), exactly as `Arc<T, A>` requires. Hence the `Send`
// bound is `A: Send + Sync`, not `std`'s `A: Send`. The `Pointee` bound
// is implicit (already on the `Box` struct).
unsafe impl<T: ?Sized + Pointee + Send, A: Allocator + Clone + Send + Sync> Send for Box<T, A> {}
// SAFETY: a `Box<T, A>` owns its pointee and the chunk `+1`, so sending it
// across threads can move the pointee and trigger chunk teardown (which may run
// `A::deallocate`) on the receiving thread; that requires `T: Send` and
// `A: Send + Sync`. Sharing `&Box<T, A>` across threads exposes only `&T`
// (`Deref` is `&self -> &T`); `DerefMut` requires `&mut self` and is serialized
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
    ///   whose storage was bump-allocated from a [`Chunk<A>`].
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
    #[expect(
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
        // SAFETY: `ptr` is hosted in a 64K-aligned `Chunk` we hold a +1
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
    /// Convert an initialized [`Box<MaybeUninit<T>, A>`] into a [`Box<T, A>`].
    ///
    /// This is O(1) — no copy,
    /// no allocation.
    ///
    /// # Safety
    ///
    /// The `MaybeUninit<T>` must contain a fully-initialized, valid `T`.
    ///
    /// ```
    /// use multitude::{Arena, Box};
    ///
    /// let arena = Arena::new();
    /// let value = arena.alloc_zeroed_box::<u32>();
    /// // SAFETY: zero is a valid `u32` representation.
    /// let value: Box<u32> = unsafe { value.assume_init() };
    /// assert_eq!(*value, 0);
    /// ```
    #[must_use]
    #[inline]
    pub unsafe fn assume_init(self) -> Box<T, A> {
        let thin = self.ptr;
        mem::forget(self);
        // SAFETY: the caller guarantees the `MaybeUninit<T>` holds an
        // initialized, valid `T`. For sized `T`, `MaybeUninit<T>` and `T` share
        // the same chunk layout (no prefix), so the thin pointer (whose chunk
        // `+1` is transferred via `mem::forget(self)`) reconstructs a valid
        // `Box<T>`.
        unsafe { Box::from_raw(thin) }
    }

    /// Convert an initialized pinned `Box<MaybeUninit<T>, A>` into `Pin<Box<T, A>>`.
    ///
    /// This is O(1).
    ///
    /// The pin is preserved across the cast: the value's address is
    /// the same `Box` allocation's address; nothing moves.
    ///
    /// # Safety
    ///
    /// The `MaybeUninit<T>` must contain a fully-initialized, valid `T`.
    ///
    /// ```
    /// use core::pin::Pin;
    ///
    /// use multitude::{Arena, Box};
    ///
    /// let arena = Arena::new();
    /// let value = Pin::new(arena.alloc_zeroed_box::<u32>());
    /// // SAFETY: zero is a valid `u32` representation.
    /// let value = unsafe { Box::assume_init_pin(value) };
    /// assert_eq!(*value, 0);
    /// ```
    #[must_use]
    #[inline]
    pub unsafe fn assume_init_pin(this: Pin<Self>) -> Pin<Box<T, A>>
    where
        A: 'static,
    {
        // SAFETY: `Pin::into_inner_unchecked` is sound because we immediately
        // re-pin the result, and the value's address is unchanged across the
        // cast (nothing moves). The caller's `assume_init` contract (the
        // `MaybeUninit<T>` holds a valid `T`) is forwarded unchanged.
        unsafe {
            let inner: Self = Pin::into_inner_unchecked(this);
            Box::into_pin(inner.assume_init())
        }
    }
}

impl<T, A: Allocator + Clone> Box<[MaybeUninit<T>], A> {
    /// Convert an initialized [`Box<[MaybeUninit<T>], A>`](crate::Box) into `Box<[T], A>`.
    ///
    /// This is O(1) —
    /// no copy, no allocation.
    ///
    /// # Safety
    ///
    /// Every element of the slice must contain a fully-initialized,
    /// valid `T`.
    ///
    /// ```
    /// use multitude::{Arena, Box};
    ///
    /// let arena = Arena::new();
    /// let values = arena.alloc_zeroed_slice_box::<u16>(3);
    /// // SAFETY: zero is a valid `u16` representation.
    /// let values: Box<[u16]> = unsafe { values.assume_init() };
    /// assert_eq!(&*values, &[0, 0, 0]);
    /// ```
    #[must_use]
    #[inline]
    pub unsafe fn assume_init(self) -> Box<[T], A> {
        let thin = self.ptr;
        mem::forget(self);
        // SAFETY: the caller guarantees every element is an initialized, valid
        // `T`. `[MaybeUninit<T>]` and `[T]` share the same chunk prefix layout
        // (slice length as `usize`), so the length stored there already matches
        // the new fat pointer's metadata, and the thin pointer (whose chunk `+1`
        // is transferred via `mem::forget(self)`) reconstructs a valid `Box<[T]>`.
        unsafe { Box::from_raw(thin) }
    }

    /// Pinned-slice variant of [`Self::assume_init_pin`]. The slice's
    /// element addresses don't change across the cast.
    ///
    /// # Safety
    ///
    /// Every element must contain a fully-initialized, valid `T`.
    ///
    /// ```
    /// use core::pin::Pin;
    ///
    /// use multitude::{Arena, Box};
    ///
    /// let arena = Arena::new();
    /// let values = Pin::new(arena.alloc_zeroed_slice_box::<u16>(2));
    /// // SAFETY: zero is a valid `u16` representation.
    /// let values = unsafe { Box::assume_init_pin_slice(values) };
    /// assert_eq!(&*values, &[0, 0]);
    /// ```
    #[must_use]
    #[inline]
    pub unsafe fn assume_init_pin_slice(this: Pin<Self>) -> Pin<Box<[T], A>>
    where
        A: 'static,
    {
        // SAFETY: `Pin::into_inner_unchecked` is sound because we immediately
        // re-pin the result, and the elements' addresses are unchanged across
        // the cast (nothing moves). The caller's slice `assume_init` contract
        // (every element is a valid `T`) is forwarded unchanged.
        unsafe {
            let inner: Self = Pin::into_inner_unchecked(this);
            Box::into_pin(inner.assume_init())
        }
    }
}

impl<T: ?Sized + Pointee, A: Allocator + Clone> DerefMut for Box<T, A> {
    #[inline]
    fn deref_mut(&mut self) -> &mut T {
        // SAFETY: `as_fat_ptr` reconstructs the fat pointer to the live pointee
        // this `Box` owns; the `&mut self` receiver proves exclusive access, so
        // handing out `&mut T` for that borrow's lifetime introduces no aliasing.
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

impl<'a, T, A: Allocator + Clone> From<Vec<'a, T, A>> for Box<[T], A> {
    /// Freeze a [`Vec`](crate::vec::Vec) into an immutable
    /// [`Box<[T], A>`](crate::Box). Mirrors `std`'s `From<Vec<T>> for Box<[T]>`.
    ///
    /// Generally **O(1)** (reuses the existing storage with no copy), except in
    /// rare edge cases where it falls back to an **O(n)** element move.
    #[inline]
    fn from(v: Vec<'a, T, A>) -> Self {
        v.into_boxed_slice()
    }
}
