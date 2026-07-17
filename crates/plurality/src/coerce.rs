// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Compiler-checked pointer unsizing for pooled handles.
//!
//! Stable Rust does not let a user-defined smart pointer participate in unsizing
//! coercions (`CoerceUnsized`/`Unsize` are unstable), so [`Box`](crate::Box),
//! [`Arc`](crate::Arc) and [`Rc`](crate::Rc) convert a sized handle into an
//! unsized one through a [`Coercion`] token instead.
//!
//! A `Coercion<T, U>` carries a function that performs the *real* compiler
//! unsizing coercion `*const T -> *const U` (for `U` a trait object or slice).
//! Because the metadata (vtable or slice length) is produced by the compiler and
//! the address and provenance of the original pointer are preserved by that same
//! coercion, [`unsize`] simply applies the function and reinterprets the result;
//! it makes no assumptions about the internal layout of fat pointers.
//!
//! Build a token with the safe [`Coercion!`](crate::Coercion!) macro (for trait
//! objects) or [`Coercion::to_slice`] (for arrays). [`Coercion::new`] is the
//! `unsafe` escape hatch for coercions the macro cannot express.

use core::marker::PhantomData;
use core::ptr::NonNull;

/// A compiler-checked proof that `*const T` can be unsized to `*const U`.
///
/// Pass one to [`Box::unsize`](crate::Box::unsize),
/// [`Arc::unsize`](crate::Arc::unsize) or [`Rc::unsize`](crate::Rc::unsize).
/// Construct it with the [`Coercion!`](crate::Coercion!) macro, with
/// [`Coercion::to_slice`], or — for coercions those cannot express — with the
/// `unsafe` [`Coercion::new`].
pub struct Coercion<T, U: ?Sized, F: FnOnce(*const T) -> *const U = fn(*const T) -> *const U> {
    coerce: F,
    _phantom: PhantomData<fn(*const T) -> *const U>,
}

impl<T, U: ?Sized, F: FnOnce(*const T) -> *const U> Coercion<T, U, F> {
    /// Wraps a coercion function in a token.
    ///
    /// The [`Coercion!`](crate::Coercion!) macro and [`Coercion::to_slice`] cover
    /// the common cases safely; reach for this only when neither fits.
    ///
    /// # Safety
    ///
    /// `coerce` must perform *only* an unsizing coercion of its argument
    /// (`ptr as *const U`) and nothing else. In particular it must return a
    /// pointer with the same address and provenance as its input, differing only
    /// by the added DST metadata. The idiomatic body is just `ptr`:
    ///
    /// ```
    /// use core::fmt::Debug;
    ///
    /// use plurality::Coercion;
    ///
    /// fn coerce(p: *const u32) -> *const dyn Debug {
    ///     p
    /// }
    /// // SAFETY: `coerce` only unsizes the pointer to a trait object.
    /// let coercion = unsafe { Coercion::new(coerce) };
    /// # let _ = coercion;
    /// ```
    #[inline]
    #[must_use]
    pub const unsafe fn new(coerce: F) -> Self {
        Self {
            coerce,
            _phantom: PhantomData,
        }
    }
}

impl<T, U: ?Sized, F: FnOnce(*const T) -> *const U> core::fmt::Debug for Coercion<T, U, F> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Coercion").finish_non_exhaustive()
    }
}

impl<T, const N: usize> Coercion<[T; N], [T]> {
    /// A coercion that unsizes an array `[T; N]` to a slice `[T]`.
    ///
    /// ```
    /// use plurality::{Box, Coercion, Pool};
    ///
    /// let pool = Pool::<[u8; 3]>::new();
    /// let slice: Box<[u8]> = Box::unsize(pool.alloc_box([1, 2, 3]), Coercion::to_slice());
    /// assert_eq!(&*slice, &[1, 2, 3]);
    /// ```
    #[must_use]
    pub fn to_slice() -> Self {
        fn coerce<T, const N: usize>(ptr: *const [T; N]) -> *const [T] {
            ptr
        }
        // SAFETY: `coerce` only unsizes the array pointer to a slice pointer.
        unsafe { Self::new(coerce) }
    }
}

/// Applies `coercion` to `ptr`, producing the unsized pointer.
///
/// The coercion function performs a genuine compiler unsizing coercion, which
/// preserves the pointer's address and provenance while attaching the DST
/// metadata, so the result points at the same value as `ptr`.
#[inline]
pub(crate) fn unsize<T, U: ?Sized, F: FnOnce(*const T) -> *const U>(ptr: NonNull<T>, coercion: Coercion<T, U, F>) -> NonNull<U> {
    let unsized_ptr = (coercion.coerce)(ptr.as_ptr());
    // SAFETY: an unsizing coercion preserves the address, and `ptr` is non-null,
    // so `unsized_ptr` is non-null.
    unsafe { NonNull::new_unchecked(unsized_ptr.cast_mut()) }
}

/// Builds a [`Coercion`](struct@crate::Coercion) that unsizes to a trait object.
///
/// The syntax mirrors a trait-object type: `Coercion!(to dyn Trait)`, including
/// bounds such as `Coercion!(to dyn Trait + Send)`.
///
/// ```
/// use core::fmt::Debug;
/// use plurality::{Box, Coercion, Pool};
///
/// let pool = Pool::<u32>::new();
/// let erased: Box<dyn Debug> = Box::unsize(pool.alloc_box(7), Coercion!(to dyn Debug));
/// assert_eq!(format!("{erased:?}"), "7");
/// ```
#[macro_export]
macro_rules! Coercion {
    (to dyn $($bounds:tt)*) => {
        // SAFETY: `coerce` only unsizes the pointer to the trait object; its body
        // is a plain compiler coercion.
        #[allow(unused_unsafe)]
        unsafe {
            $crate::Coercion::new({
                #[allow(unused_parens)]
                fn coerce<'lt>(ptr: *const (impl $($bounds)* + 'lt)) -> *const (dyn $($bounds)* + 'lt) {
                    ptr
                }
                coerce
            })
        }
    };
}
