// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use allocator_api2::alloc::{Allocator, Global};

use crate::arena_str_macros::{impl_utf16_str_accessors, impl_utf16_str_handle_core, impl_utf16_str_read_traits};
use crate::internal::in_chunk::InLocalChunk;
use crate::internal::local_chunk::LocalChunk;

/// An immutable, single-pointer reference-counted UTF-16 string stored
/// in an [`Arena`](crate::Arena).
///
/// 8 bytes on 64-bit (one pointer); contrast with `&'arena Utf16Str`'s
/// 16 bytes. Cloning is **O(1)** (a non-atomic refcount bump). For
/// cross-thread sharing, use [`ArcUtf16Str`](crate::strings::ArcUtf16Str)
/// instead.
///
/// `RcUtf16Str` is the recommended long-term storage type for arena
/// UTF-16 strings. Build via either:
///
/// - [`Arena::alloc_utf16_str_rc`](crate::Arena::alloc_utf16_str_rc) —
///   copy an `&Utf16Str` directly into the arena.
/// - [`Utf16String`](crate::strings::Utf16String) +
///   [`into_arena_utf16_str`](crate::strings::Utf16String::into_arena_utf16_str)
///   — build incrementally, then freeze.
///
/// # Example
///
/// ```
/// # #[cfg(feature = "utf16")] {
/// use multitude::Arena;
/// use widestring::utf16str;
///
/// let arena = Arena::new();
/// let s = arena.alloc_utf16_str_rc(utf16str!("hello"));
/// assert_eq!(&*s, utf16str!("hello"));
/// assert_eq!(s.len(), 5);
/// # }
/// ```
pub struct RcUtf16Str<A: Allocator + Clone = Global> {
    data: InLocalChunk<u16, A>,
    _phantom: core::marker::PhantomData<A>,
}

impl_utf16_str_handle_core!(RcUtf16Str, Local);
impl_utf16_str_accessors!([<A: Allocator + Clone>], RcUtf16Str<A>);
impl_utf16_str_read_traits!([<A: Allocator + Clone>], RcUtf16Str<A>);

impl<A: Allocator + Clone> Drop for RcUtf16Str<A> {
    #[inline]
    fn drop(&mut self) {
        let chunk = self.data.chunk_ptr();
        // SAFETY: refcount-positive invariant — this handle owns +1.
        unsafe { LocalChunk::dec_ref(chunk) };
    }
}

impl<'a, A: Allocator + Clone> ::core::convert::From<crate::strings::Utf16String<'a, A>> for RcUtf16Str<A> {
    /// Freeze an [`Utf16String`](crate::strings::Utf16String) into an
    /// immutable [`RcUtf16Str<A>`]. See
    /// [`Utf16String::into_arena_utf16_str`](crate::strings::Utf16String::into_arena_utf16_str).
    #[inline]
    fn from(s: crate::strings::Utf16String<'a, A>) -> Self {
        s.into_arena_utf16_str()
    }
}

crate::arena_str_macros::impl_from_str_for_slice_handle!(RcUtf16Str, Rc, u16);
