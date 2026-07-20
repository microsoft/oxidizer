// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![expect(
    clippy::multiple_unsafe_ops_per_block,
    reason = "pointer-recovery and slot-lifecycle paths group tightly-coupled unsafe operations under a single documented safety invariant; one block per operation would duplicate that invariant and obscure it"
)]

use core::marker::PhantomData;
use core::mem::MaybeUninit;
use core::ops::{Deref, DerefMut};
use core::pin::Pin;
use core::ptr::NonNull;

use allocator_api2::alloc::{Allocator, Global};

use crate::coerce::{self, Coercion};
use crate::common::{impl_handle_common_unsized, impl_handle_mut_unsized};
use crate::pool::drop_and_free_val;
use crate::slot::SlotCell;

/// A unique, owning handle to a value in a [`Pool`](crate::Pool).
///
/// Derefs to `&T`/`&mut T`; dropping it runs `T`'s destructor and returns the
/// slot to the pool. `Box` may outlive the `Pool` handle.
///
/// `Box` is generic over `T: ?Sized`, so it can own an unsized value — a trait
/// object (`Box<dyn Trait>`) or a slice (`Box<[U]>`) — obtained from a sized
/// `Box` via [`Box::unsize`]. A `Box` of a `Sized` `T` is exactly **one pointer
/// wide**; the unsized forms carry the extra pointer metadata (vtable or length)
/// just like [`alloc::boxed::Box`], and the slot's bookkeeping is recovered from
/// the value's runtime size and alignment on drop.
pub struct Box<T: ?Sized, A: Allocator = Global> {
    /// Pointer to the **value** (field 0 of its `SlotCell<T>`). The value never
    /// moves, so this is stable. For a `Sized` `T` this is one word; for an
    /// unsized `T` it is a fat pointer. The free path recovers the slot's
    /// bookkeeping from the value's size/align.
    slot: NonNull<T>,
    _marker: PhantomData<A>,
}

// SAFETY: a `Box` is the unique owner of its slot; the pool machinery it touches
// on drop is atomic. Sending requires `T: Send` and a thread-safe allocator.
unsafe impl<T: ?Sized + Send, A: Allocator + Send + Sync> Send for Box<T, A> {}
// SAFETY: `&Box` only exposes `&T`, so sharing needs `T: Sync`.
unsafe impl<T: ?Sized + Sync, A: Allocator + Send + Sync> Sync for Box<T, A> {}

impl<T, A: Allocator> Box<T, A> {
    #[inline]
    pub(crate) fn from_slot(slot: NonNull<SlotCell<T>>) -> Self {
        Self {
            // The value is field 0 of `SlotCell<T>`, so its address is the slot
            // address.
            slot: slot.cast::<T>(),
            _marker: PhantomData,
        }
    }

    /// Erases a `Box<T>` into a `Box<U>` for an unsized `U` (e.g. `dyn Trait` or
    /// a slice), keeping the value in its pool slot.
    ///
    /// The [`Coercion`](struct@Coercion) token proves that `T` can be unsized to `U` while
    /// preserving the value's address and provenance. Use the [`coerce!`](crate::coerce!)
    /// macro for arbitrary trait objects or a provided constructor such as
    /// [`Coercion::to_slice`].
    ///
    /// ```
    /// use core::fmt::Debug;
    ///
    /// use plurality::{Box, Pool, coerce};
    ///
    /// let pool = Pool::<u32>::new();
    /// let b = pool.alloc_box(7u32);
    /// let dyn_b: Box<dyn Debug> = Box::unsize::<dyn Debug>(b, coerce!(dyn Debug));
    /// assert_eq!(format!("{dyn_b:?}"), "7");
    /// ```
    ///
    /// The returned `Box<U>` owns the same slot; dropping it runs `T`'s
    /// destructor (through `U`'s metadata) and returns the slot to the pool.
    ///
    /// A coercion token operates on the raw value pointer so the slot's full
    /// provenance is retained for reclamation.
    #[must_use]
    pub fn unsize<U: ?Sized>(this: Self, coercion: Coercion<T, U, impl FnOnce(*const T) -> *const U>) -> Box<U, A> {
        let value = coerce::unsize(this.slot, coercion);
        // The returned box inherits ownership of the same slot.
        core::mem::forget(this);
        Box::from_value(value)
    }
}

impl<T: ?Sized, A: Allocator> Box<T, A> {
    #[inline]
    pub(crate) fn from_value(value: NonNull<T>) -> Self {
        Self {
            slot: value,
            _marker: PhantomData,
        }
    }

    /// Consumes the handle and returns the raw value pointer **without** freeing
    /// the slot, leaving the value alive and the slot occupied.
    ///
    /// This is the escape hatch for manual lifetime management (mirroring
    /// [`alloc::boxed::Box::into_raw`]): ownership of the slot is transferred to
    /// the returned pointer. The value is not dropped and the slot is not
    /// returned to the pool until the pointer is handed back to
    /// [`from_raw`](Self::from_raw). The pointer stays valid and stable (the
    /// value never moves) until then.
    #[must_use]
    #[inline]
    pub fn into_raw(this: Self) -> NonNull<T> {
        let ptr = this.slot;
        // Suppress the drop so the slot stays occupied; the caller now owns it.
        core::mem::forget(this);
        ptr
    }

    /// Reconstructs a `Box` from a pointer produced by [`into_raw`](Self::into_raw),
    /// taking back ownership of the slot.
    ///
    /// # Safety
    /// `ptr` must be the exact pointer returned by [`into_raw`](Self::into_raw),
    /// including unchanged metadata for a slice or trait object, and the
    /// reconstructed `Box` must use the same allocator type `A` as the original.
    /// The pointer must be passed to `from_raw` **exactly once**. After this call
    /// the reconstructed `Box` owns the slot; dropping it runs the value's
    /// destructor and returns the slot to the pool.
    #[must_use]
    #[inline]
    pub unsafe fn from_raw(ptr: NonNull<T>) -> Self {
        Self::from_value(ptr)
    }

    /// Borrows the owned value as a [`Pin<&mut T>`].
    ///
    /// The value's address is fixed and mutable access that could move a
    /// `!Unpin` value is unavailable. This is useful for repeatedly polling a
    /// pooled `Future` without consuming the handle.
    #[must_use]
    #[inline]
    pub fn as_pin_mut(&mut self) -> Pin<&mut T> {
        // SAFETY: the slot is uniquely owned and stable, and `DerefMut`/`AsMut`
        // are implemented only when `T: Unpin`.
        unsafe { Pin::new_unchecked(self.slot.as_mut()) }
    }
}

impl<T, A: Allocator> Box<MaybeUninit<T>, A> {
    #[inline]
    #[cfg_attr(test, mutants::skip)] // Removing initialization makes assume_init() UB, so the mutant is invalid.
    pub(crate) fn write_value(&mut self, value: T) {
        // SAFETY: allocation APIs call this only for a freshly reserved,
        // uniquely owned uninitialized slot.
        unsafe { self.slot.as_mut().write(value) };
    }

    /// Converts an uninitialized box into an initialized one.
    ///
    /// # Safety
    /// The value must have been fully initialized (e.g. via
    /// [`MaybeUninit::write`]) before calling.
    #[must_use]
    pub unsafe fn assume_init(self) -> Box<T, A> {
        let value = self.slot.cast::<T>();
        // Don't run the uninit box's destructor; transfer the slot as-is.
        core::mem::forget(self);
        Box::from_value(value)
    }

    /// Converts a pinned, uninitialized box into a pinned, initialized one.
    ///
    /// # Safety
    /// The value must have been fully initialized before calling.
    #[must_use]
    pub unsafe fn assume_init_pin(this: Pin<Self>) -> Pin<Box<T, A>> {
        // SAFETY: the caller guarantees the value is initialized; the slot
        // address is unchanged, so re-pinning is sound.
        unsafe {
            let inner = Pin::into_inner_unchecked(this);
            Pin::new_unchecked(inner.assume_init())
        }
    }
}

impl<T: ?Sized, A: Allocator> Deref for Box<T, A> {
    type Target = T;

    #[inline]
    fn deref(&self) -> &T {
        // SAFETY: the slot is occupied for as long as this `Box` is alive.
        unsafe { self.slot.as_ref() }
    }
}

impl<T: ?Sized + Unpin, A: Allocator> DerefMut for Box<T, A> {
    #[inline]
    fn deref_mut(&mut self) -> &mut T {
        // SAFETY: this `Box` is the unique owner of the occupied slot.
        unsafe { self.slot.as_mut() }
    }
}

impl<T: ?Sized, A: Allocator> Drop for Box<T, A> {
    #[inline]
    fn drop(&mut self) {
        // SAFETY: unique owner; the last (only) handle to the occupied slot.
        unsafe { drop_and_free_val::<T>(self.slot) };
    }
}

impl_handle_common_unsized!(Box);
impl_handle_mut_unsized!(Box);
