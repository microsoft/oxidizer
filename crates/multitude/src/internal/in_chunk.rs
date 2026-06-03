// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Pointer-into-chunk-payload smart pointer.

use core::marker::PhantomData;
use core::ptr::NonNull;

/// A non-null, well-aligned pointer that — by construction — addresses
/// storage inside the payload of a live arena chunk (with one narrow
/// exception for ZSTs, see below).
///
/// `InChunk<T>` is the fundamental "I came from the allocator" pointer
/// abstraction. The rest of the crate carries these around instead of raw
/// `NonNull<T>` so that the difference between "any pointer" and "a pointer
/// the allocator handed out" is visible in the type system.
///
/// # Invariants
///
/// - `self.ptr` is non-null and well-aligned for `T`.
/// - If `core::mem::size_of_val(&*self.ptr) > 0`, the pointed-to region lies
///   entirely within the payload of an arena chunk whose lifetime exceeds the
///   use of this `InChunk`. (Liveness is enforced externally by the holder of
///   the chunk's `Arc`.)
/// - For zero-sized values (ZSTs and empty slices) the pointer is permitted
///   to be a dangling, well-aligned non-null address. There is no payload
///   storage to reference in that case.
///
/// `InChunk` is `Copy` because copying a pointer cannot violate any of the
/// above. Mutability and aliasing discipline are enforced by the wrappers
/// (`Uninit`, `UninitDrop`, `ArenaBuf`, etc.) that consume `InChunk`s.
pub(crate) struct InChunk<T: ?Sized> {
    ptr: NonNull<T>,
    _phantom: PhantomData<*const T>,
}

impl<T: ?Sized> Clone for InChunk<T> {
    #[inline]
    #[cfg_attr(coverage_nightly, coverage(off))]
    fn clone(&self) -> Self {
        *self
    }
}

impl<T: ?Sized> Copy for InChunk<T> {}

impl<T: ?Sized> InChunk<T> {
    /// Wraps a raw `NonNull<T>` that satisfies the type invariants above.
    ///
    /// This constructor is `pub(super)` so only sibling modules in
    /// `internal/` (notably `ChunkMutator`) can mint `InChunk` values; the
    /// rest of the crate may only obtain them through allocator outputs.
    #[inline]
    pub(super) fn from_raw(ptr: NonNull<T>) -> Self {
        Self {
            ptr,
            _phantom: PhantomData,
        }
    }

    /// Returns the underlying raw pointer.
    #[inline]
    pub(crate) fn as_ptr(self) -> *mut T {
        self.ptr.as_ptr()
    }

    /// Returns the underlying non-null pointer.
    #[inline]
    pub(crate) fn as_non_null(self) -> NonNull<T> {
        self.ptr
    }
}

impl<T: ?Sized> InChunk<T> {
    /// Reinterprets the pointer as pointing at a `U` instead of a `T`.
    ///
    /// The caller is responsible for ensuring the target type `U` is a valid
    /// interpretation of the same storage (matching size and alignment).
    #[inline]
    pub(crate) fn cast<U>(self) -> InChunk<U> {
        InChunk {
            ptr: self.ptr.cast(),
            _phantom: PhantomData,
        }
    }
}

impl InChunk<u8> {
    /// Returns the integer address of the pointer.
    #[inline]
    pub(crate) fn addr(self) -> usize {
        self.ptr.as_ptr() as usize
    }

    /// Builds an `InChunk<[T]>` describing `len` consecutive `T`s starting at
    /// this byte address.
    ///
    /// The caller (always `ChunkMutator`) is responsible for ensuring that
    /// the address is aligned for `T` and that `len * size_of::<T>()` bytes
    /// of valid in-chunk storage start here.
    #[inline]
    pub(crate) fn into_slice<T>(self, len: usize) -> InChunk<[T]> {
        let slice = NonNull::slice_from_raw_parts(self.ptr.cast::<T>(), len);
        InChunk {
            ptr: slice,
            _phantom: PhantomData,
        }
    }
}
