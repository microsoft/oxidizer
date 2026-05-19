// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use allocator_api2::alloc::{Allocator, Global};

use crate::arena_str_macros::{impl_utf16_str_accessors, impl_utf16_str_handle_core, impl_utf16_str_read_traits};
use crate::internal::in_chunk::InSharedChunk;
use crate::internal::shared_chunk::SharedChunk;

/// An immutable, single-pointer reference-counted UTF-16 string stored
/// in an [`Arena`](crate::Arena), safe to share across threads.
///
/// 8 bytes on 64-bit (one pointer); contrast with `&'arena Utf16Str`'s
/// 16 bytes. Cloning is **O(1)** (one atomic refcount bump). For
/// single-threaded code, prefer [`RcUtf16Str`](crate::strings::RcUtf16Str) — it
/// has the same layout and same cost model with a non-atomic refcount.
///
/// Build via either:
///
/// - [`Arena::alloc_utf16_str_arc`](crate::Arena::alloc_utf16_str_arc) —
///   copy an `&Utf16Str` directly into the arena.
/// - [`Utf16String`](crate::strings::Utf16String) +
///   [`into_arena_utf16_str`](crate::strings::Utf16String::into_arena_utf16_str),
///   then convert with `.into()` — build incrementally, then freeze.
///
/// # Example
///
/// ```
/// # #[cfg(feature = "utf16")] {
/// use std::thread;
///
/// use multitude::Arena;
/// use widestring::utf16str;
///
/// let arena = Arena::new();
/// let s = arena.alloc_utf16_str_arc(utf16str!("shared"));
/// let s2 = s.clone();
/// let h = thread::spawn(move || s2.len());
/// assert_eq!(s.len(), h.join().unwrap());
/// # }
/// ```
pub struct ArcUtf16Str<A: Allocator + Clone = Global> {
    data: InSharedChunk<u16, A>,
    _phantom: core::marker::PhantomData<A>,
}

// SAFETY: backed by a Shared chunk with atomic refcount.
unsafe impl<A: Allocator + Clone + Send + Sync> Send for ArcUtf16Str<A> {}
// SAFETY: see above.
unsafe impl<A: Allocator + Clone + Send + Sync> Sync for ArcUtf16Str<A> {}

impl_utf16_str_handle_core!(ArcUtf16Str, Shared);
impl_utf16_str_accessors!([<A: Allocator + Clone>], ArcUtf16Str<A>);
impl_utf16_str_read_traits!([<A: Allocator + Clone>], ArcUtf16Str<A>);

impl<A: Allocator + Clone> Drop for ArcUtf16Str<A> {
    #[inline]
    fn drop(&mut self) {
        let chunk = self.data.chunk_ptr();
        // SAFETY: refcount-positive invariant — this handle owns one logical refcount.
        unsafe { SharedChunk::dec_ref(chunk) };
    }
}

crate::arena_str_macros::impl_from_str_for_slice_handle!(ArcUtf16Str, Arc, u16);
