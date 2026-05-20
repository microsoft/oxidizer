// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use core::marker::PhantomData;
use core::ptr::NonNull;

use allocator_api2::alloc::{Allocator, Global};

use crate::arena_str_macros::{impl_box_utf16_str_extras, impl_utf16_str_accessors, impl_utf16_str_read_traits};
use crate::internal::in_chunk::InLocalChunk;
use crate::strings::RcUtf16Str;

/// An owned, mutable, single-pointer UTF-16 string stored in an
/// [`Arena`](crate::Arena).
///
/// 8 bytes on 64-bit (one pointer). Unlike
/// [`RcUtf16Str`](crate::strings::RcUtf16Str) /
/// [`ArcUtf16Str`](crate::strings::ArcUtf16Str):
///
/// - Provides `&mut Utf16Str` through `DerefMut`.
/// - **Not** [`Clone`] — single owner.
///
/// `BoxUtf16Str` keeps its containing chunk alive by holding a +1
/// refcount on it, so the smart pointer can outlive the arena it came
/// from and survives [`Arena::reset`](crate::Arena::reset). For shared
/// storage after the build phase, freeze into an
/// [`RcUtf16Str`](crate::strings::RcUtf16Str) via [`Self::into_rc_utf16_str`].
///
/// # Example
///
/// ```
/// # #[cfg(feature = "utf16")] {
/// use multitude::Arena;
/// use widestring::utf16str;
///
/// let arena = Arena::new();
/// let mut s = arena.alloc_utf16_str_box(utf16str!("hello"));
/// assert_eq!(s.len(), 5);
/// assert_eq!(&*s, utf16str!("hello"));
/// // `&mut Utf16Str` access is available via DerefMut / `as_mut_utf16_str`.
/// let _: &mut widestring::Utf16Str = &mut *s;
/// # }
/// ```
pub struct BoxUtf16Str<A: Allocator + Clone = Global> {
    data: InLocalChunk<u16, A>,
    _phantom: PhantomData<A>,
}

impl<A: Allocator + Clone> BoxUtf16Str<A> {
    /// # Safety
    ///
    /// `data` must point at valid in-chunk UTF-16 data, and this handle must
    /// own the chunk's `+1` ref.
    #[inline]
    pub(crate) const unsafe fn from_raw_data(data: NonNull<u16>) -> Self {
        // SAFETY: caller forwards the in-local-chunk invariant.
        unsafe { Self::from_in_chunk(InLocalChunk::new(data)) }
    }

    /// Typed counterpart of [`Self::from_raw_data`].
    ///
    /// # Safety
    ///
    /// Same contract as [`Self::from_raw_data`].
    #[inline]
    pub(crate) const unsafe fn from_in_chunk(data: InLocalChunk<u16, A>) -> Self {
        Self {
            data,
            _phantom: PhantomData,
        }
    }

    /// Construct from an [`OwnedInLocalChunk`] that already owns the in-chunk
    /// invariant and `+1` ref.
    #[inline]
    pub(crate) fn from_owned_in_chunk(owned: crate::internal::owned_in_chunk::OwnedInLocalChunk<u16, A>) -> Self {
        Self {
            data: owned.into_in_chunk(),
            _phantom: PhantomData,
        }
    }

    /// Pointer to the first `u16` element.
    #[inline]
    pub(crate) const fn data_ptr(&self) -> NonNull<u16> {
        self.data.as_non_null()
    }
}

impl_utf16_str_accessors!([<A: Allocator + Clone>], BoxUtf16Str<A>);
impl_utf16_str_read_traits!([<A: Allocator + Clone>], BoxUtf16Str<A>);
impl_box_utf16_str_extras!(BoxUtf16Str, RcUtf16Str);
