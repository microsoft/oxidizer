// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Trait-object coercion tokens for arena smart pointers.

use core::fmt;
use core::marker::PhantomData;
use core::ptr::NonNull;

use ptr_meta::Pointee;

/// A proof that `*const T` can be unsized to `*const U`.
///
/// Pass one to [`Box::unsize`](crate::Box::unsize),
/// [`Arc::unsize`](crate::Arc::unsize), or [`Rc::unsize`](crate::Rc::unsize).
/// [`coerce!`](crate::coerce!) constructs compiler-checked tokens.
/// Tokens constructed with [`Coercion::new`] rely on its unsafe caller
/// contract instead.
pub struct Coercion<T, U: ?Sized, F: FnOnce(*const T) -> *const U = fn(*const T) -> *const U> {
    coerce: F,
    _phantom: PhantomData<fn(*const T) -> *const U>,
}

impl<T, U: ?Sized, F: FnOnce(*const T) -> *const U> Coercion<T, U, F> {
    /// Wraps a coercion function in a token.
    ///
    /// The [`coerce!`](crate::coerce!) macro covers ordinary trait-object
    /// coercions safely.
    ///
    /// # Safety
    ///
    /// `coerce` must perform only an unsizing coercion of its argument and
    /// return a pointer with the same address and provenance, differing only
    /// by trait-object metadata.
    #[inline]
    #[must_use]
    pub const unsafe fn new(coerce: F) -> Self {
        Self {
            coerce,
            _phantom: PhantomData,
        }
    }
}

impl<T, U: ?Sized, F: FnOnce(*const T) -> *const U> fmt::Debug for Coercion<T, U, F> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Coercion").finish_non_exhaustive()
    }
}

/// Applies `coercion` and returns the resulting pointer metadata.
#[inline]
pub(crate) fn unsize_metadata<T, U: ?Sized + Pointee, F: FnOnce(*const T) -> *const U>(
    ptr: NonNull<u8>,
    coercion: Coercion<T, U, F>,
) -> U::Metadata {
    let source = ptr.cast::<T>().as_ptr().cast_const();
    let target = (coercion.coerce)(source);
    assert!(
        core::ptr::addr_eq(source, target),
        "Coercion::new contract violated: coercion function changed the pointer address"
    );
    ptr_meta::metadata(target)
}

/// Builds a [`Coercion`] that unsizes to a trait object.
///
/// The syntax is `coerce!(dyn Trait)`. If the trait object refers to an
/// enclosing generic type, bind it explicitly with
/// `coerce!(<T> dyn Trait<T>)`.
///
/// Multitude's smart-pointer consumers require the target to implement
/// [`ptr_meta::Pointee`] with [`ptr_meta::DynMetadata`]. The dependency
/// provides this for [`core::any::Any`] and [`core::error::Error`] trait
/// objects, including their `Send` / `Sync` combinations. Custom traits
/// require the `dst` feature and `#[multitude::dst::pointee]`.
///
/// The attribute implements `Pointee` for plain `dyn LocalTrait`, not for
/// separate spellings such as `dyn LocalTrait + Send`. Put required auto
/// traits in the trait's supertraits and coerce to plain `dyn LocalTrait`.
/// Foreign trait objects without a `ptr_meta` implementation, such as
/// `dyn Debug`, cannot currently be Multitude smart-pointer targets.
///
/// ```
/// use core::any::Any;
///
/// use multitude::{Arena, Box, coerce};
///
/// let arena = Arena::new();
/// let erased: Box<dyn Any> = Box::unsize(arena.alloc_box(7_u32), coerce!(dyn Any));
/// assert_eq!(erased.downcast_ref::<u32>(), Some(&7));
/// ```
///
/// ```compile_fail
/// use core::fmt::Debug;
///
/// use multitude::{Arena, Box, coerce};
///
/// let arena = Arena::new();
/// let _: Box<dyn Debug> = Box::unsize(arena.alloc_box(7_u32), coerce!(dyn Debug));
/// ```
///
/// ```compile_fail
/// use multitude::{Arena, Box, coerce};
///
/// #[multitude::dst::pointee]
/// trait LocalTrait {}
///
/// impl LocalTrait for u32 {}
///
/// let arena = Arena::new();
/// let _: Box<dyn LocalTrait + Send> =
///     Box::unsize(arena.alloc_box(7_u32), coerce!(dyn LocalTrait + Send));
/// ```
#[macro_export]
macro_rules! coerce {
    (<$($generic:ident),+> dyn $($bounds:tt)*) => {
        // SAFETY: `coerce` performs only the compiler's pointer unsizing
        // coercion.
        #[allow(unused_unsafe)]
        unsafe {
            $crate::Coercion::new({
                #[allow(unused_parens)]
                fn coerce<'lt, $($generic),+>(
                    ptr: *const (impl $($bounds)* + 'lt),
                ) -> *const (dyn $($bounds)* + 'lt) {
                    ptr
                }
                coerce::<$($generic),+>
            })
        }
    };
    (dyn $($bounds:tt)*) => {
        // SAFETY: `coerce` performs only the compiler's pointer unsizing
        // coercion.
        #[allow(unused_unsafe)]
        unsafe {
            $crate::Coercion::new({
                #[allow(unused_parens)]
                fn coerce<'lt>(
                    ptr: *const (impl $($bounds)* + 'lt),
                ) -> *const (dyn $($bounds)* + 'lt) {
                    ptr
                }
                coerce
            })
        }
    };
}
