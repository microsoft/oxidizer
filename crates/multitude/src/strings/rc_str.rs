// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use allocator_api2::alloc::{Allocator, Global};

use crate::arena_str_macros::{impl_str_accessors, impl_str_handle_core, impl_str_read_traits};
use crate::internal::in_chunk::InLocalChunk;
use crate::internal::local_chunk::LocalChunk;

/// An immutable, single-pointer reference-counted UTF-8 string stored in
/// an [`Arena`](crate::Arena).
///
/// 8 bytes on 64-bit (one pointer); contrast with `&'arena str`'s 16
/// bytes. Cloning is **O(1)** (a non-atomic refcount bump). For
/// cross-thread sharing, use [`ArcStr`](crate::strings::ArcStr) instead.
///
/// `RcStr` is the recommended long-term storage type for arena strings.
/// Build via either:
///
/// - [`Arena::alloc_str_rc`](crate::Arena::alloc_str_rc) — copy a `&str`
///   directly into the arena.
/// - [`String`](crate::strings::String) +
///   [`into_arena_str`](crate::strings::String::into_arena_str) — build
///   incrementally, then freeze.
///
/// # Example
///
/// ```
/// use multitude::Arena;
/// use multitude::strings::RcStr;
///
/// let arena = Arena::new();
/// let s = arena.alloc_str_rc("hello");
/// assert_eq!(&*s, "hello");
/// assert_eq!(s.len(), 5);
/// ```
pub struct RcStr<A: Allocator + Clone = Global> {
    data: InLocalChunk<u8, A>,
    _phantom: core::marker::PhantomData<A>,
}

impl_str_handle_core!(RcStr, Local);
impl_str_accessors!([<A: Allocator + Clone>], RcStr<A>);
impl_str_read_traits!([<A: Allocator + Clone>], RcStr<A>);

impl<A: Allocator + Clone> Drop for RcStr<A> {
    #[inline]
    fn drop(&mut self) {
        let chunk = self.data.chunk_ptr();
        // SAFETY: refcount-positive invariant — this handle owns +1.
        unsafe { LocalChunk::dec_ref(chunk) };
    }
}

impl<'a, A: Allocator + Clone> ::core::convert::From<crate::strings::String<'a, A>> for RcStr<A> {
    /// Freeze an [`String`](crate::strings::String) into an immutable
    /// [`RcStr<A>`]. See [`String::into_arena_str`](crate::strings::String::into_arena_str).
    #[inline]
    fn from(s: crate::strings::String<'a, A>) -> Self {
        s.into_arena_str()
    }
}

crate::arena_str_macros::impl_from_str_for_slice_handle!(RcStr, Rc, u8);
