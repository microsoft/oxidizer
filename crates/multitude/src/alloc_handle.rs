// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! [`Alloc`]: an arena-lifetime owning handle to a single bump allocation.

use core::borrow::{Borrow, BorrowMut};
use core::hash::{Hash, Hasher};
use core::mem::ManuallyDrop;
use core::ops::{Deref, DerefMut};
use core::{fmt, ptr};

/// An owning handle to a value tied to its [`Arena`](crate::Arena).
///
/// # Memory reclamation
///
/// Dropping an `Alloc` runs the destructor but does **not** return the slot to
/// the bump cursor (a bump allocator can only rewind its cursor, not free
/// interior slots). The memory is reclaimed in bulk at the next
/// [`Arena::reset`](crate::Arena::reset) or when the arena is dropped — the same
/// as for any other arena allocation.
///
/// Leaking an `Alloc` with [`core::mem::forget`] leaks the destructor: the
/// value's `Drop` never runs. This is sound (no use-after-free), but the value
/// is simply never finalized.
///
/// Unlike [`crate::Box`], `Alloc` does not support pinning. A forgotten handle
/// does not retain its backing chunk, so the arena may reclaim or reuse the
/// slot without running the destructor for the pointee.
///
/// ```compile_fail
/// use core::pin::Pin;
/// use multitude::{Alloc, Arena};
///
/// let arena = Arena::new();
/// let _: Pin<Alloc<'_, u32>> = arena.alloc(1_u32).into();
/// ```
///
/// ```compile_fail
/// use core::marker::PhantomPinned;
/// use core::pin::Pin;
/// use multitude::Arena;
///
/// let arena = Arena::new();
/// let _ = Pin::new(arena.alloc(PhantomPinned));
/// ```
///
/// # `Send` and `Sync`
///
/// `Alloc<'a, T>` is [`Send`] when `T: Send` and [`Sync`] when `T: Sync`,
/// inherited directly from its `&'a mut T` field.
///
/// # Example
///
/// ```
/// use multitude::Arena;
///
/// let arena = Arena::new();
/// let mut x = arena.alloc(41);
/// *x += 1;
/// assert_eq!(*x, 42);
/// ```
pub struct Alloc<'a, T: ?Sized> {
    /// Exclusive borrow of the arena storage holding the value. The borrow
    /// binds `Alloc` to the arena lifetime and makes [`Deref`] / [`DerefMut`]
    /// safe; ownership of the value (for `Drop`) is conveyed by the API
    /// contract that the arena hands each slot out exactly once.
    inner: &'a mut T,
}

impl<'a, T: ?Sized> Alloc<'a, T> {
    /// Wraps an exclusive arena borrow into an owning `Alloc` handle.
    ///
    /// # Safety
    ///
    /// `inner` must be the *unique* reference to a fully-initialized value in
    /// arena storage that lives for at least `'a` and whose destructor has not
    /// yet been (and will not otherwise be) run. The resulting `Alloc` takes
    /// ownership of the value and runs its destructor exactly once on drop.
    #[inline]
    pub(crate) unsafe fn from_mut(inner: &'a mut T) -> Self {
        Self { inner }
    }

    /// Returns the mutable borrow without running the value's destructor.
    ///
    /// This is the escape hatch back to a bare arena-lifetime reference: the
    /// value lives until the arena is reset or dropped, and its destructor is
    /// never run.
    ///
    /// ```
    /// use multitude::{Alloc, Arena};
    ///
    /// let arena = Arena::new();
    /// let value = Alloc::leak(arena.alloc(String::from("kept")));
    /// value.push_str(" alive");
    /// assert_eq!(value, "kept alive");
    /// ```
    #[must_use]
    #[inline]
    pub fn leak(this: Self) -> &'a mut T {
        // Moving the stored reference preserves its borrow tag.
        let this = ManuallyDrop::new(this);
        // SAFETY: `this.inner` is a valid, initialized `&'a mut T`; `ptr::read`
        // moves it out exactly once without running `Drop`.
        unsafe { ptr::read(&raw const this.inner) }
    }
}

impl<T: ?Sized> Deref for Alloc<'_, T> {
    type Target = T;

    #[inline]
    fn deref(&self) -> &T {
        self.inner
    }
}

impl<T: ?Sized> DerefMut for Alloc<'_, T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut T {
        self.inner
    }
}

impl<T: ?Sized> Drop for Alloc<'_, T> {
    #[inline]
    fn drop(&mut self) {
        // SAFETY: `Alloc` uniquely owns this slot (the arena handed it out
        // exactly once), the value is initialized, and `self.inner` is never
        // touched again after this, so the value is dropped exactly once.
        unsafe { ptr::drop_in_place(self.inner) };
    }
}

impl<T: ?Sized> AsRef<T> for Alloc<'_, T> {
    #[inline]
    fn as_ref(&self) -> &T {
        self.inner
    }
}

impl<T: ?Sized> AsMut<T> for Alloc<'_, T> {
    #[inline]
    fn as_mut(&mut self) -> &mut T {
        self.inner
    }
}

impl<T: ?Sized> Borrow<T> for Alloc<'_, T> {
    #[inline]
    fn borrow(&self) -> &T {
        self.inner
    }
}

impl<T: ?Sized> BorrowMut<T> for Alloc<'_, T> {
    #[inline]
    fn borrow_mut(&mut self) -> &mut T {
        self.inner
    }
}

impl<T: ?Sized + fmt::Debug> fmt::Debug for Alloc<'_, T> {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&**self, f)
    }
}

impl<T: ?Sized + fmt::Display> fmt::Display for Alloc<'_, T> {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&**self, f)
    }
}

impl<T: ?Sized> fmt::Pointer for Alloc<'_, T> {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let ptr: *const T = self.inner;
        fmt::Pointer::fmt(&ptr, f)
    }
}

impl<T: ?Sized + PartialEq> PartialEq for Alloc<'_, T> {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        PartialEq::eq(&**self, &**other)
    }
}

impl<T: ?Sized + Eq> Eq for Alloc<'_, T> {}

impl<T: ?Sized + PartialOrd> PartialOrd for Alloc<'_, T> {
    #[inline]
    fn partial_cmp(&self, other: &Self) -> Option<core::cmp::Ordering> {
        PartialOrd::partial_cmp(&**self, &**other)
    }
}

impl<T: ?Sized + Ord> Ord for Alloc<'_, T> {
    #[inline]
    fn cmp(&self, other: &Self) -> core::cmp::Ordering {
        Ord::cmp(&**self, &**other)
    }
}

impl<T: ?Sized + Hash> Hash for Alloc<'_, T> {
    #[inline]
    fn hash<H: Hasher>(&self, state: &mut H) {
        (**self).hash(state);
    }
}
