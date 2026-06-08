// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use core::marker::PhantomData;
use core::ptr::NonNull;

use allocator_api2::alloc::{Allocator, Global};
use widestring::Utf16Str;

use crate::internal::chunk_ref::ChunkRef;
use crate::strings::utf16_str_common::impl_utf16_str_common;

/// An immutable, single-pointer reference-counted UTF-16 string stored
/// in an [`Arena`](crate::Arena), safe to share across threads.
///
/// **8 bytes** on 64-bit (one pointer). The pointer addresses the first
/// `u16` of the UTF-16 payload inside a 64K-aligned shared chunk; the
/// element count is stored as a `usize` immediately before the payload
/// (read with [`core::ptr::read_unaligned`], no usize-alignment padding
/// imposed on the chunk).
///
/// Cloning is **O(1)** — one atomic refcount bump on the hosting
/// chunk. Lengths and indexing are in `u16` code units.
pub struct ArcUtf16Str<A: Allocator + Clone = Global> {
    /// Thin pointer to the first `u16` of the payload. The element
    /// count lives in the `usize` immediately preceding the payload
    /// (read with `read_unaligned`).
    ptr: NonNull<u16>,
    _phantom: PhantomData<(*const Utf16Str, A)>,
}

// SAFETY: thin pointer into an atomically-refcounted shared chunk;
// `Utf16Str` is `Send + Sync`.
unsafe impl<A: Allocator + Clone + Send + Sync> Send for ArcUtf16Str<A> {}
// SAFETY: exposes only `&Utf16Str` (shared, immutable) — no interior
// mutability, no `&mut`-yielding API.
unsafe impl<A: Allocator + Clone + Send + Sync> Sync for ArcUtf16Str<A> {}

impl<A: Allocator + Clone> ArcUtf16Str<A> {
    /// Builds an `ArcUtf16Str` from a raw length-prefixed payload pointer.
    ///
    /// # Safety
    ///
    /// - `ptr` must point at the first `u16` of a length-prefixed UTF-16
    ///   payload bump-allocated from a `SharedChunk<A>` (via
    ///   [`Arena::impl_alloc_prefixed_shared`](crate::Arena)).
    /// - The caller must have just acquired a +1 refcount on that chunk
    ///   in the new `ArcUtf16Str`'s name; the returned value owns that
    ///   +1 and releases it in [`Drop`].
    /// - `ptr` must lie within the first `CHUNK_ALIGN` bytes of the
    ///   chunk so the header-from-mask helper recovers the chunk address.
    #[inline]
    pub(crate) unsafe fn from_raw(ptr: NonNull<u16>) -> Self {
        Self {
            ptr,
            _phantom: PhantomData,
        }
    }
}

impl_utf16_str_common!(ArcUtf16Str);

impl<A: Allocator + Clone> Clone for ArcUtf16Str<A> {
    #[inline]
    fn clone(&self) -> Self {
        // SAFETY: `self` owns a live +1 on its chunk so the chunk is
        // alive; `clone_from_value_ptr` mints a fresh +1 via an
        // atomic bump and returns a `ChunkRef` that owns it.
        let r: ChunkRef<A> = unsafe { ChunkRef::clone_from_value_ptr(self.ptr) };
        let _ = r.forget();
        Self {
            ptr: self.ptr,
            _phantom: PhantomData,
        }
    }
}

impl<A: Allocator + Clone> From<ArcUtf16Str<A>> for crate::Arc<[u16], A> {
    /// Convert an [`ArcUtf16Str<A>`] into an [`Arc<[u16], A>`](crate::Arc).
    ///
    /// `ArcUtf16Str` is a thin 8-byte pointer to a length-prefixed
    /// UTF-16 payload in a shared chunk; this reads the length from
    /// the chunk prefix, reconstructs a `NonNull<[u16]>` over the same
    /// payload, and transfers the chunk +1 into the fat
    /// `Arc<[u16], A>`. O(1), no copy.
    #[inline]
    fn from(s: ArcUtf16Str<A>) -> Self {
        use core::mem::ManuallyDrop;
        let me = ManuallyDrop::new(s);
        // SAFETY: `me.ptr` was produced by
        // `Arena::impl_alloc_prefixed_shared::<u16>`; it carries
        // chunk-wide provenance, the prefix word stores the u16
        // element count, and `ManuallyDrop` transfers the chunk +1
        // into the new `Arc<[u16]>` (whose `as_fat_ptr` recovers the
        // length from the same prefix on demand).
        unsafe { Self::from_raw(me.ptr.cast::<u8>()) }
    }
}
