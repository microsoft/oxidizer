// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use core::borrow::BorrowMut;
use core::marker::PhantomData;
use core::ops::DerefMut;
use core::ptr::NonNull;
use core::slice;

use allocator_api2::alloc::{Allocator, Global};
use widestring::Utf16Str;

use crate::strings::utf16_str_common::impl_utf16_str_common;

/// An owned, mutable, single-pointer UTF-16 string stored in an
/// [`Arena`](crate::Arena).
///
/// **8 bytes** on 64-bit (one pointer). The pointer addresses the first
/// `u16` of the UTF-16 payload inside a 64K-aligned shared chunk; the
/// element count is stored as a `usize` immediately before the payload
/// (read with [`core::ptr::read_unaligned`], no usize-alignment padding
/// imposed on the chunk). Lengths and indexing are in `u16` code units.
///
/// # `Send` and `Sync`
///
/// `BoxUtf16Str<A>` is [`Send`] when `A: Send` and [`Sync`] when
/// `A: Sync` — `Utf16Str` is itself `Send + Sync`. The backing chunk's
/// refcount is atomic, so dropping the `BoxUtf16Str` on a thread other
/// than the one that created it is sound.
pub struct BoxUtf16Str<A: Allocator + Clone = Global> {
    /// Thin pointer to the first `u16` of the payload. The element
    /// count lives in the `usize` immediately preceding the payload.
    pub(crate) ptr: NonNull<u16>,
    _phantom: PhantomData<(*const Utf16Str, A)>,
}

// SAFETY: thin pointer into an atomically-refcounted shared chunk;
// `Utf16Str` is `Send + Sync`.
unsafe impl<A: Allocator + Clone + Send> Send for BoxUtf16Str<A> {}
// SAFETY: `&BoxUtf16Str` exposes only `&Utf16Str` (immutable);
// `DerefMut` requires `&mut self` and is serialized by the borrow
// checker.
unsafe impl<A: Allocator + Clone + Sync> Sync for BoxUtf16Str<A> {}

impl<A: Allocator + Clone> BoxUtf16Str<A> {
    /// Builds a `BoxUtf16Str` from a raw length-prefixed payload pointer.
    ///
    /// # Safety
    ///
    /// Same contract as [`crate::strings::ArcUtf16Str::from_raw`].
    #[inline]
    pub(crate) unsafe fn from_raw(ptr: NonNull<u16>) -> Self {
        Self {
            ptr,
            _phantom: PhantomData,
        }
    }

    /// Borrow as `&mut Utf16Str`.
    #[must_use]
    #[inline]
    pub fn as_mut_utf16_str(&mut self) -> &mut Utf16Str {
        let len = self.u16_len();
        // SAFETY: `&mut self` grants exclusive access to the payload
        // for the returned borrow's lifetime; `ptr` addresses `len`
        // initialized valid UTF-16 units.
        unsafe {
            let units = slice::from_raw_parts_mut(self.ptr.as_ptr(), len);
            Utf16Str::from_slice_unchecked_mut(units)
        }
    }
}

impl_utf16_str_common!(BoxUtf16Str);

impl<A: Allocator + Clone> DerefMut for BoxUtf16Str<A> {
    #[inline]
    fn deref_mut(&mut self) -> &mut Utf16Str {
        self.as_mut_utf16_str()
    }
}

impl<A: Allocator + Clone> AsMut<Utf16Str> for BoxUtf16Str<A> {
    #[inline]
    fn as_mut(&mut self) -> &mut Utf16Str {
        self.as_mut_utf16_str()
    }
}

impl<A: Allocator + Clone> BorrowMut<Utf16Str> for BoxUtf16Str<A> {
    #[inline]
    fn borrow_mut(&mut self) -> &mut Utf16Str {
        self.as_mut_utf16_str()
    }
}
