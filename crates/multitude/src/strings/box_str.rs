// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use core::marker::PhantomData;
use core::ptr::NonNull;

use allocator_api2::alloc::{Allocator, Global};

use crate::arena_str_macros::{impl_box_str_extras, impl_str_accessors, impl_str_read_traits};
use crate::internal::in_chunk::InLocalChunk;
use crate::strings::RcStr;

/// An owned, mutable, single-pointer UTF-8 string stored in an
/// [`Arena`](crate::Arena).
///
/// 8 bytes on 64-bit (one pointer). Unlike [`RcStr`](crate::strings::RcStr) /
/// [`ArcStr`](crate::strings::ArcStr):
///
/// - Provides `&mut str` through `DerefMut`.
/// - **Not** [`Clone`] — single owner.
///
/// `BoxStr` keeps its containing chunk alive by holding a +1 refcount on
/// it, so the smart pointer can outlive the arena it came from and
/// survives [`Arena::reset`](crate::Arena::reset). For shared,
/// refcounted, immutable storage after the build phase, freeze into an
/// [`RcStr`](crate::strings::RcStr) via [`Self::into_rc_str`]. Like
/// `BoxStr` itself, `RcStr` is `!Send`/`!Sync` — both live in `Local`
/// chunks and can only be accessed from the arena's owning thread.
///
/// # Example
///
/// ```
/// use multitude::Arena;
///
/// let arena = Arena::new();
/// let mut s = arena.alloc_str_box("hello");
/// // Mutable in place:
/// s.make_ascii_uppercase();
/// assert_eq!(&*s, "HELLO");
/// ```
pub struct BoxStr<A: Allocator + Clone = Global> {
    data: InLocalChunk<u8, A>,
    _phantom: PhantomData<A>,
}

impl<A: Allocator + Clone> BoxStr<A> {
    /// # Safety
    ///
    /// `data` must point at valid in-chunk UTF-8 data, and this handle must
    /// own the chunk's `+1` ref.
    #[inline]
    pub(crate) const unsafe fn from_raw_data(data: NonNull<u8>) -> Self {
        // SAFETY: caller forwards the in-local-chunk invariant.
        unsafe { Self::from_in_chunk(InLocalChunk::new(data)) }
    }

    /// Typed counterpart of [`Self::from_raw_data`].
    ///
    /// # Safety
    ///
    /// Same contract as [`Self::from_raw_data`].
    #[inline]
    pub(crate) const unsafe fn from_in_chunk(data: InLocalChunk<u8, A>) -> Self {
        Self {
            data,
            _phantom: PhantomData,
        }
    }

    /// Construct from an [`OwnedInLocalChunk`] that already owns the in-chunk
    /// invariant and `+1` ref.
    #[inline]
    pub(crate) fn from_owned_in_chunk(owned: crate::internal::owned_in_chunk::OwnedInLocalChunk<u8, A>) -> Self {
        Self {
            data: owned.into_in_chunk(),
            _phantom: PhantomData,
        }
    }

    /// Pointer to the first data byte.
    #[inline]
    pub(crate) const fn data_ptr(&self) -> NonNull<u8> {
        self.data.as_non_null()
    }
}

impl_str_accessors!([<A: Allocator + Clone>], BoxStr<A>);
impl_str_read_traits!([<A: Allocator + Clone>], BoxStr<A>);
impl_box_str_extras!(BoxStr, RcStr);
