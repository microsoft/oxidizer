// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! String-specific impls for `Arc<Utf16Str>` / `Box<Utf16Str>`.
//!
//! The generic `Arc<T>` / `Box<T>` impls already cover `Clone`, `Drop`,
//! `Deref<Target = Utf16Str>` (which in turn derefs to
//! `widestring::Utf16Str`), `AsRef`, `Borrow`, `Debug`, `Display`,
//! `PartialEq`, `Eq`, `PartialOrd`, `Ord`, `Hash`, `Pointer`, `Unpin`, and
//! `DerefMut`/`AsMut` for `Box`. This module adds the parts specific to
//! UTF-16 strings: comparison against a bare `Utf16Str`, the zero-copy retag
//! to `[u16]`, and a string-flavoured `serde::Serialize`.

use core::borrow::{Borrow, BorrowMut};
use core::mem::ManuallyDrop;

use allocator_api2::alloc::Allocator;
use widestring::Utf16Str as WideUtf16Str;

use super::Utf16Str;
use crate::{Arc, Box};

macro_rules! impl_utf16_str_smart_ptr_extras {
    ($Ty:ident) => {
        impl<A: Allocator + Clone> PartialEq<WideUtf16Str> for $Ty<Utf16Str, A> {
            #[inline]
            fn eq(&self, other: &WideUtf16Str) -> bool {
                (**self).as_widestring_utf16_str() == other
            }
        }

        impl<A: Allocator + Clone> PartialEq<&WideUtf16Str> for $Ty<Utf16Str, A> {
            #[inline]
            fn eq(&self, other: &&WideUtf16Str) -> bool {
                (**self).as_widestring_utf16_str() == *other
            }
        }

        // The generic `Arc<T>`/`Box<T>` give `AsRef`/`Borrow` over the
        // `Utf16Str` *newtype*; these expose the underlying
        // `widestring::Utf16Str` (e.g. for `HashMap<_, _>` keyed by it).
        impl<A: Allocator + Clone> AsRef<WideUtf16Str> for $Ty<Utf16Str, A> {
            #[inline]
            fn as_ref(&self) -> &WideUtf16Str {
                (**self).as_widestring_utf16_str()
            }
        }

        impl<A: Allocator + Clone> Borrow<WideUtf16Str> for $Ty<Utf16Str, A> {
            #[inline]
            fn borrow(&self) -> &WideUtf16Str {
                (**self).as_widestring_utf16_str()
            }
        }

        #[cfg(feature = "serde")]
        #[cfg_attr(docsrs, doc(cfg(feature = "serde")))]
        impl<A: Allocator + Clone> serde::ser::Serialize for $Ty<Utf16Str, A> {
            fn serialize<S: serde::ser::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.collect_str((**self).as_widestring_utf16_str())
            }
        }
    };
}

impl_utf16_str_smart_ptr_extras!(Arc);
impl_utf16_str_smart_ptr_extras!(Box);

impl<A: Allocator + Clone> AsMut<WideUtf16Str> for Box<Utf16Str, A> {
    #[inline]
    fn as_mut(&mut self) -> &mut WideUtf16Str {
        (**self).as_mut_widestring_utf16_str()
    }
}

impl<A: Allocator + Clone> BorrowMut<WideUtf16Str> for Box<Utf16Str, A> {
    #[inline]
    fn borrow_mut(&mut self) -> &mut WideUtf16Str {
        (**self).as_mut_widestring_utf16_str()
    }
}

impl<A: Allocator + Clone> From<Arc<Utf16Str, A>> for Arc<[u16], A> {
    /// Convert an `Arc<Utf16Str, A>` into an [`Arc<[u16], A>`](crate::Arc)
    /// without copying.
    ///
    /// `Utf16Str` is `#[repr(transparent)]` over `[u16]` and shares the same
    /// length-prefixed chunk layout, so this only retags the element type
    /// and transfers the chunk `+1` (and strong count) into the new handle.
    #[inline]
    fn from(s: Arc<Utf16Str, A>) -> Self {
        let me = ManuallyDrop::new(s);
        // SAFETY: `Utf16Str` and `[u16]` share layout, metadata (the `usize`
        // element count) and the strong-count prefix; `ManuallyDrop`
        // transfers the chunk `+1` and `thin_ptr` keeps chunk-wide
        // provenance.
        unsafe { Self::from_raw(me.thin_ptr()) }
    }
}

impl<A: Allocator + Clone> From<Box<Utf16Str, A>> for Box<[u16], A> {
    /// Convert a `Box<Utf16Str, A>` into a [`Box<[u16], A>`](crate::Box)
    /// without copying. See [`From<Arc<Utf16Str, A>> for Arc<[u16], A>`].
    #[inline]
    fn from(s: Box<Utf16Str, A>) -> Self {
        let me = ManuallyDrop::new(s);
        // SAFETY: see the `Arc` retag above (no strong count for `Box`).
        unsafe { Self::from_raw(me.thin_ptr()) }
    }
}
