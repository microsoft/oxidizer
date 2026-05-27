// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![expect(
    clippy::type_repetition_in_bounds,
    clippy::cast_sign_loss,
    reason = "trait-impl `where` clauses are kept uniform across all forwarding impls; numeric casts are bounded by upstream `usize` checks documented at call sites"
)]

use core::borrow::{Borrow, BorrowMut};
use core::cmp::Ordering;
use core::fmt::{self, Debug, Display, Formatter, Pointer};
use core::hash::{Hash, Hasher};
use core::iter::FusedIterator;
use core::marker::PhantomData;
use core::mem::{MaybeUninit, forget, needs_drop};
use core::ops::{Deref, DerefMut};
use core::pin::Pin;
use core::ptr::{NonNull, drop_in_place, slice_from_raw_parts_mut};
use core::sync::atomic::Ordering as AtomicOrdering;

use allocator_api2::alloc::{Allocator, Global};

use crate::internal::drop_list::{drop_shim_one, drop_shim_slice};
use crate::internal::in_chunk::InLocalChunk;
use crate::internal::local_chunk::LocalChunk;
use crate::rc::Rc;

/// An owned, mutable smart pointer to a `T` stored in an
/// [`Arena`](crate::Arena).
///
/// Created via [`Arena::alloc_box`](crate::Arena::alloc_box).
///
/// Unlike [`Rc`](crate::Rc) / [`Arc`](crate::Arc):
///
/// - **Drop runs when the smart pointer is dropped**, not at chunk teardown. Useful for
///   `T`s that hold OS resources which must be released promptly.
/// - Provides `&mut T` through `DerefMut`.
/// - **Not** [`Clone`] — single owner.
///
/// Like [`Rc`](crate::Rc), `Box` keeps its containing
/// chunk alive by holding a +1 refcount, so it can outlive the arena it
/// came from and survives [`Arena::reset`](crate::Arena::reset).
///
/// # `Send` and `Sync`
///
/// [`Box`] is **always `!Send` and `!Sync`** — even when `T` itself is
/// both. This is intentional: a [`Box`] holds a refcount on a `Local`
/// (non-atomic) chunk, and the chunk's refcount must only be touched
/// from the arena's owning thread. Sending a [`Box`] across threads
/// would let two threads race on that non-atomic counter, breaking
/// soundness.
///
/// If you need `Send` + `Sync` ownership of an arena value, use
/// [`Arc`](crate::Arc) (via
/// [`Arena::alloc_arc`](crate::Arena::alloc_arc)) instead. Convert from
/// a [`Box<T, A>`] to an [`Rc<T, A>`](crate::Rc) via
/// [`Box::into_rc`](Self::into_rc) when you need shared, immutable
/// access (also `!Send`/`!Sync`).
///
/// # Pinning
///
/// `Box` implements [`Unpin`] unconditionally (like `std::Box`).
/// Pinning a `Box` is sound: because `Box` holds a +1 refcount on its
/// chunk, the backing memory **cannot** be freed or reused while the
/// `Box` exists.  If a pinned `Box` is leaked via [`core::mem::forget`],
/// the refcount is never decremented and the chunk's storage persists
/// for the lifetime of the process — satisfying [`Pin`](core::pin::Pin)'s drop
/// guarantee (the pinned value's memory is never reclaimed).
///
/// # Panics during `T::drop`
///
/// If `T::drop` panics, [`Box`] still releases its chunk hold during
/// unwinding (a refcount-release guard runs after `drop_in_place` even
/// on panic). This matches `std::Box`'s value-level semantics but at
/// chunk granularity. **Caveat:** if a panic occurs *while already
/// unwinding* (i.e., a destructor panics during another panic's
/// stack unwind), Rust aborts the process per its standard
/// double-panic rule, and no further cleanup runs.
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
pub struct Box<T: ?Sized, A: Allocator + Clone = Global> {
    ptr: InLocalChunk<T, A>,
    _phantom: PhantomData<(*const T, A)>,
}

impl<T, A: Allocator + Clone> Box<T, A> {
    /// # Safety
    ///
    /// `ptr` must point to an initialized `T` in a local arena chunk,
    /// and that chunk must already hold this box's `+1`.
    ///
    /// A drop entry is only needed if the box might later convert to
    /// `Rc`/`Arc`; plain `Box::drop` runs `drop_in_place` directly.
    #[must_use]
    #[inline]
    pub(crate) const unsafe fn from_raw(ptr: NonNull<T>) -> Self {
        // SAFETY: caller forwards the in-local-chunk invariant.
        unsafe { Self::from_in_chunk(InLocalChunk::new(ptr)) }
    }

    /// Like [`Self::from_raw`] for an already-validated in-chunk pointer.
    ///
    /// # Safety
    ///
    /// Same contract as [`Self::from_raw`]. A drop entry is only needed
    /// for later `Rc`/`Arc` conversion.
    #[must_use]
    #[inline]
    pub(crate) const unsafe fn from_in_chunk(ptr: InLocalChunk<T, A>) -> Self {
        Self {
            ptr,
            _phantom: PhantomData,
        }
    }

    /// Build a `Box` from an [`OwnedInLocalChunk`] that already proves
    /// the in-chunk invariant and owns the `+1`.
    #[must_use]
    #[inline]
    pub(crate) fn from_owned_in_chunk(owned: crate::internal::owned_in_chunk::OwnedInLocalChunk<T, A>) -> Self {
        Self {
            ptr: owned.into_in_chunk(),
            _phantom: PhantomData,
        }
    }

    /// Convert this owned, mutable box into a shared, immutable
    /// [`Rc<T, A>`](crate::Rc). O(1) — no copy, no allocation.
    ///
    /// # Example
    ///
    /// ```
    /// use multitude::{Arena, Rc};
    ///
    /// let arena = Arena::new();
    /// let mut b = arena.alloc_box(vec![1, 2, 3]);
    /// b.push(4);
    /// // Done mutating — freeze and share.
    /// let rc: Rc<Vec<i32>> = b.into_rc();
    /// let rc2 = rc.clone();
    /// assert_eq!(*rc, *rc2);
    /// ```
    ///
    /// # Panics
    ///
    /// Panics for `T: Drop` if the chunk does not have a drop entry
    /// for this allocation. This indicates the `Box` was constructed
    /// via a path that did not reserve one (e.g.
    /// `alloc_box(MaybeUninit::new(...))` followed by `assume_init`);
    /// use [`Arena::alloc_uninit_box`](crate::Arena::alloc_uninit_box)
    /// instead.
    #[must_use]
    #[inline]
    pub fn into_rc(self) -> Rc<T, A> {
        let in_chunk = self.ptr;
        let value_ptr = in_chunk.as_non_null();
        // If `T: Drop`, hand destructor ownership back to the chunk.
        if needs_drop::<T>() {
            retarget_box_drop_entry::<A>(self.chunk(), value_ptr.cast::<u8>(), drop_shim_one::<T>);
        }
        forget(self);
        // SAFETY: the chunk refcount transfers from `Box` to `Rc`; `in_chunk`
        // already encodes the in-local-chunk invariant.
        unsafe { Rc::from_in_chunk(in_chunk) }
    }
}

impl<T: ?Sized, A: Allocator + Clone> Box<T, A> {
    /// # Safety
    ///
    /// Same contract as [`Self::from_raw`], but for a possibly-unsized
    /// `T`. The fat pointer must already carry valid metadata.
    #[must_use]
    #[inline]
    pub(crate) const unsafe fn from_raw_unsized(ptr: NonNull<T>) -> Self {
        // SAFETY: caller forwards the in-local-chunk invariant.
        unsafe { Self::from_in_chunk_unsized(InLocalChunk::new(ptr)) }
    }

    /// Like [`Self::from_raw_unsized`] for an already-validated in-chunk pointer.
    ///
    /// # Safety
    ///
    /// Same contract as [`Self::from_raw_unsized`].
    #[must_use]
    #[inline]
    pub(crate) const unsafe fn from_in_chunk_unsized(ptr: InLocalChunk<T, A>) -> Self {
        Self {
            ptr,
            _phantom: PhantomData,
        }
    }

    /// Construct an unsized `Box` from an [`OwnedInLocalChunk`] that
    /// already carries both the in-chunk invariant and the `+1`
    /// refcount ownership for this `Box`. Safe at the call site.
    #[must_use]
    #[inline]
    pub(crate) fn from_owned_in_chunk_unsized(owned: crate::internal::owned_in_chunk::OwnedInLocalChunk<T, A>) -> Self {
        Self {
            ptr: owned.into_in_chunk(),
            _phantom: PhantomData,
        }
    }

    /// Borrow the containing chunk header while this `Box` keeps it alive.
    #[inline]
    fn chunk(&self) -> &LocalChunk<A> {
        // SAFETY: `self.ptr` is in a live local chunk, held by this box's `+1`.
        unsafe { self.ptr.chunk_ptr().as_ref() }
    }

    /// Returns a raw pointer to the value.
    #[must_use]
    #[inline]
    pub const fn as_ptr(&self) -> *const T {
        self.ptr.as_ptr()
    }

    /// Returns a raw mutable pointer to the value.
    #[expect(
        clippy::needless_pass_by_ref_mut,
        reason = "associated-fn convention (like alloc::rc::Rc::as_ptr); &mut self conveys exclusive access"
    )]
    #[must_use]
    #[inline]
    pub const fn as_mut_ptr(this: &mut Self) -> *mut T {
        this.ptr.as_ptr()
    }

    /// Convert this `Box<T, A>` into a [`Pin<Box<T, A>>`](core::pin::Pin).
    ///
    /// Sound for any `T` (including `!Unpin`) because `Box` is the
    /// unique owner of its value, the value's address is fixed at
    /// allocation time, and `Box::drop` runs `drop_in_place` at that
    /// same address — satisfying `Pin`'s contract.
    ///
    /// Mirrors `std::boxed::Box::into_pin`. Use this when you need
    /// to convert an existing `Box` into a pinned handle; allocate
    /// directly into a pinned `Box` via
    /// [`Arena::alloc_box_pin`](crate::Arena::alloc_box_pin) or
    /// [`Arena::alloc_box_pin_with`](crate::Arena::alloc_box_pin_with).
    #[must_use]
    #[inline]
    pub fn into_pin(boxed: Self) -> Pin<Self> {
        // SAFETY: Box uniquely owns the storage; the value's address
        // never changes between allocation and drop.
        unsafe { Pin::new_unchecked(boxed) }
    }
}

impl<T: ?Sized, A: Allocator + Clone> From<Box<T, A>> for Pin<Box<T, A>> {
    /// Mirror of `From<std::boxed::Box<T>> for Pin<std::boxed::Box<T>>`.
    /// See [`Box::into_pin`] for the soundness argument.
    #[inline]
    fn from(boxed: Box<T, A>) -> Self {
        Box::into_pin(boxed)
    }
}

// No `leak`: dropping the refcount risks UAF; keeping it leaks the chunk.

impl<T, A: Allocator + Clone> Box<[T], A> {
    /// Convert this owned, mutable
    /// slice box into a shared, immutable [`Rc<[T], A>`](crate::Rc).
    /// O(1) — no copy, no allocation.
    ///
    /// # Example
    ///
    /// ```
    /// let arena = multitude::Arena::new();
    /// let mut b = arena.alloc_slice_copy_box([1_u32, 2, 3]);
    /// b[1] = 99;
    /// let rc = b.into_rc();
    /// assert_eq!(&*rc, &[1, 99, 3]);
    /// ```
    ///
    /// # Panics
    ///
    /// Panics for `T: Drop` if the chunk does not have a drop entry
    /// for this allocation. See [`Box::into_rc`] for details.
    #[must_use]
    #[inline]
    #[cfg_attr(test, mutants::skip)] // Empty or non-drop slices have no entry to retarget, so the mutation is a no-op.
    pub fn into_rc(self) -> Rc<[T], A> {
        let in_chunk = self.ptr;
        let value_ptr = in_chunk.as_non_null();
        let len = value_ptr.len();
        // If `T: Drop`, hand destructor ownership back to the chunk.
        // Empty slices have no entry.
        if needs_drop::<T>() && len > 0 {
            retarget_box_drop_entry::<A>(self.chunk(), value_ptr.cast::<u8>(), drop_shim_slice::<T>);
        }
        forget(self);
        // SAFETY: the chunk refcount transfers from `Box` to `Rc`; `in_chunk`
        // already encodes the in-local-chunk invariant.
        unsafe { Rc::from_in_chunk(in_chunk) }
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
        let ptr = self.ptr.as_non_null();
        forget(self);
        // SAFETY: caller guarantees initialization; this only retypes the pointer.
        unsafe { Box::from_raw(ptr.cast::<T>()) }
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
        // SAFETY: the allocation does not move across the cast;
        // `assume_init`'s contract is the caller's.
        unsafe {
            let inner = Pin::into_inner_unchecked(this);
            Pin::new_unchecked(inner.assume_init())
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
        let ptr = self.ptr.as_non_null();
        let len = ptr.len();
        forget(self);
        let data = ptr.as_ptr().cast::<T>();
        let fat = slice_from_raw_parts_mut(data, len);
        // SAFETY: `data` is non-null, the slice constructor preserves
        // that, `forget(self)` transfers the `+1`, and the caller
        // guarantees every element is initialized.
        unsafe { Box::from_raw_unsized(NonNull::new_unchecked(fat)) }
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
        // SAFETY: the cast does not move the allocation;
        // `assume_init`'s contract is the caller's.
        unsafe {
            let inner = Pin::into_inner_unchecked(this);
            Pin::new_unchecked(inner.assume_init())
        }
    }
}

impl<T: ?Sized, A: Allocator + Clone> Drop for Box<T, A> {
    #[inline]
    #[expect(
        clippy::items_after_statements,
        reason = "the release-guard helper struct is local to the slow-path arm and reads better inline"
    )]
    fn drop(&mut self) {
        let chunk = self.ptr.chunk_ptr();

        // Release the chunk's `+1` even if `T::drop` panics.
        struct ReleaseGuard<A: Allocator + Clone>(NonNull<LocalChunk<A>>);
        impl<A: Allocator + Clone> Drop for ReleaseGuard<A> {
            fn drop(&mut self) {
                // SAFETY: refcount-positive invariant — the Box owns a +1.
                unsafe { LocalChunk::dec_ref(self.0) };
            }
        }
        let _guard = ReleaseGuard::<A>(chunk);

        // SAFETY: self.ptr points at a valid T; we have exclusive access.
        unsafe { drop_in_place(self.ptr.as_ptr()) };
    }
}

impl<T: ?Sized, A: Allocator + Clone> DerefMut for Box<T, A> {
    #[inline]
    fn deref_mut(&mut self) -> &mut T {
        // SAFETY: refcount-positive invariant — we own +1, value is live.
        unsafe { self.ptr.as_mut() }
    }
}

impl<T: ?Sized, A: Allocator + Clone> AsMut<T> for Box<T, A> {
    #[inline]
    fn as_mut(&mut self) -> &mut T {
        self
    }
}

impl<T: ?Sized, A: Allocator + Clone> BorrowMut<T> for Box<T, A> {
    #[inline]
    fn borrow_mut(&mut self) -> &mut T {
        self
    }
}

impl<T, A: Allocator + Clone> From<Box<T, A>> for Rc<T, A> {
    /// Convert an [`Box<T, A>`] into an [`Rc<T, A>`]. O(1) — see
    /// [`Box::into_rc`].
    #[inline]
    fn from(b: Box<T, A>) -> Self {
        b.into_rc()
    }
}

impl<T, A: Allocator + Clone> From<Box<[T], A>> for Rc<[T], A> {
    /// Convert an [`Box<[T], A>`](crate::Box) into an [`Rc<[T], A>`](crate::Rc). O(1) — see
    /// [`Box::into_rc`].
    #[inline]
    fn from(b: Box<[T], A>) -> Self {
        b.into_rc()
    }
}

impl<T: ?Sized, A: Allocator + Clone> Deref for Box<T, A> {
    type Target = T;
    #[inline]
    fn deref(&self) -> &T {
        // SAFETY: refcount-positive invariant — we own +1, value is live.
        unsafe { self.ptr.as_ref() }
    }
}

impl<T: ?Sized, A: Allocator + Clone> Debug for Box<T, A>
where
    T: Debug,
{
    #[inline]
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        Debug::fmt(&**self, f)
    }
}

impl<T: ?Sized, A: Allocator + Clone> Display for Box<T, A>
where
    T: Display,
{
    #[inline]
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        Display::fmt(&**self, f)
    }
}

impl<T: ?Sized, A: Allocator + Clone> PartialEq for Box<T, A>
where
    T: PartialEq,
{
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        **self == **other
    }
}

impl<T: ?Sized, A: Allocator + Clone> Eq for Box<T, A> where T: Eq {}

impl<T: ?Sized, A: Allocator + Clone> PartialOrd for Box<T, A>
where
    T: PartialOrd,
{
    #[inline]
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        (**self).partial_cmp(&**other)
    }
}

impl<T: ?Sized, A: Allocator + Clone> Ord for Box<T, A>
where
    T: Ord,
{
    #[inline]
    fn cmp(&self, other: &Self) -> Ordering {
        (**self).cmp(&**other)
    }
}

impl<T: ?Sized, A: Allocator + Clone> Hash for Box<T, A>
where
    T: Hash,
{
    #[inline]
    fn hash<H: Hasher>(&self, state: &mut H) {
        (**self).hash(state);
    }
}

impl<T: ?Sized, A: Allocator + Clone> AsRef<T> for Box<T, A> {
    #[inline]
    fn as_ref(&self) -> &T {
        self
    }
}

impl<T: ?Sized, A: Allocator + Clone> Borrow<T> for Box<T, A> {
    #[inline]
    fn borrow(&self) -> &T {
        self
    }
}

impl<T: ?Sized, A: Allocator + Clone> Pointer for Box<T, A> {
    #[inline]
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        Pointer::fmt(&self.ptr.as_ptr(), f)
    }
}

impl<I: Iterator + ?Sized, A: Allocator + Clone> Iterator for Box<I, A> {
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

impl<I: DoubleEndedIterator + ?Sized, A: Allocator + Clone> DoubleEndedIterator for Box<I, A> {
    #[inline]
    fn next_back(&mut self) -> Option<Self::Item> {
        (**self).next_back()
    }

    #[inline]
    fn nth_back(&mut self, n: usize) -> Option<Self::Item> {
        (**self).nth_back(n)
    }
}

impl<I: ExactSizeIterator + ?Sized, A: Allocator + Clone> ExactSizeIterator for Box<I, A> {
    #[inline]
    fn len(&self) -> usize {
        (**self).len()
    }
}

impl<I: FusedIterator + ?Sized, A: Allocator + Clone> FusedIterator for Box<I, A> {}

impl<T: ?Sized, A: Allocator + Clone> Unpin for Box<T, A> {}

/// Retarget the pre-installed `noop_drop_shim` entry for `value_ptr` in
/// the local chunk to `drop_fn`. Walks the chunk's drop back-stack to
/// find the entry whose `value_offset` matches `value_ptr` and rewrites
/// only the entry's `drop_fn` field (8 bytes).
///
/// `alloc_box` installs a placeholder drop entry for `T: Drop`; `into_rc`
/// and `into_arc` retarget that entry to the real drop shim.
///
/// `drop_fn` must match the value at `value_ptr`.
///
/// This panics if the allocation never reserved a drop entry,
/// because silently skipping `T::drop` would leak.
fn retarget_box_drop_entry<A: Allocator + Clone>(chunk: &LocalChunk<A>, value_ptr: NonNull<u8>, drop_fn: unsafe fn(*mut u8, usize)) {
    let data = chunk.data();
    // SAFETY: `value_ptr` points into `chunk`'s payload, so `offset_from`
    // stays within one allocation and yields a non-negative offset.
    let value_offset = unsafe { value_ptr.as_ptr().offset_from(data.as_ptr()) } as usize;
    if let Some(entry) = chunk.drop_entries().iter().find(|e| e.value_offset as usize == value_offset) {
        entry.store_drop_fn(drop_fn, AtomicOrdering::Relaxed);
        return;
    }
    // Missing entries mean the allocation never reserved one for later
    // `Rc`/`Arc` conversion. Panic rather than silently skip `T::drop`.
    #[expect(
        clippy::panic,
        reason = "intentional: surface the contract violation rather than leak T::drop silently"
    )]
    {
        panic!(
            "Box::into_rc: no drop entry reserved for this allocation. \
             Use `Arena::alloc_uninit_box::<T>()` so the entry is installed eagerly; \
             `alloc_box(MaybeUninit::new(...))` does not reserve one."
        );
    }
}
