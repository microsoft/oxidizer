// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use allocator_api2::alloc::{Allocator, Global};

use crate::arena_str_macros::{impl_str_accessors, impl_str_handle_core, impl_str_read_traits};
use crate::internal::in_chunk::InSharedChunk;
use crate::internal::shared_chunk::SharedChunk;

/// An immutable, single-pointer reference-counted UTF-8 string stored in
/// an [`Arena`](crate::Arena), safe to share across threads.
///
/// 8 bytes on 64-bit (one pointer); contrast with `&'arena str`'s 16
/// bytes. Cloning is **O(1)** (one atomic refcount bump). For
/// single-threaded code, prefer [`RcStr`](crate::strings::RcStr) — it has the
/// same layout and same cost model with a non-atomic refcount.
///
/// Build via either:
///
/// - [`Arena::alloc_str_arc`](crate::Arena::alloc_str_arc) — copy a
///   `&str` directly into the arena.
/// - [`String`](crate::strings::String) +
///   [`into_arena_str`](crate::strings::String::into_arena_str), then
///   convert with `.into()` — build incrementally, then freeze.
///
/// # Example
///
/// ```
/// use std::thread;
///
/// use multitude::Arena;
/// use multitude::strings::ArcStr;
///
/// let arena = Arena::new();
/// let s = arena.alloc_str_arc("shared");
/// let s2 = s.clone();
/// let h = thread::spawn(move || s2.len());
/// assert_eq!(s.len(), h.join().unwrap());
/// ```
pub struct ArcStr<A: Allocator + Clone = Global> {
    data: InSharedChunk<u8, A>,
    _phantom: core::marker::PhantomData<A>,
}

// SAFETY: Shared chunks use atomic refcounts; `ArcStr` is `Send`/`Sync` under the same bounds.
unsafe impl<A: Allocator + Clone + Send + Sync> Send for ArcStr<A> {}
// SAFETY: see above.
unsafe impl<A: Allocator + Clone + Send + Sync> Sync for ArcStr<A> {}

impl_str_handle_core!(ArcStr, Shared);
impl_str_accessors!([<A: Allocator + Clone>], ArcStr<A>);
impl_str_read_traits!([<A: Allocator + Clone>], ArcStr<A>);

impl<A: Allocator + Clone> Drop for ArcStr<A> {
    #[inline]
    fn drop(&mut self) {
        let chunk = self.data.chunk_ptr();
        // SAFETY: refcount-positive invariant — this handle owns one logical refcount.
        unsafe { SharedChunk::dec_ref(chunk) };
    }
}

crate::arena_str_macros::impl_from_str_for_slice_handle!(ArcStr, Arc, u8);
