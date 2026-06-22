// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Pointer-into-chunk-payload smart pointer.

use core::marker::PhantomData;
use core::ptr::NonNull;

/// A non-null, well-aligned pointer produced by the chunk allocator.
///
/// # Invariants
///
/// - `self.ptr` is non-null and well-aligned for `T`.
/// - If the pointed-to region has nonzero size, it lies entirely within the
///   payload of a live arena chunk.
/// - For zero-sized values (ZSTs and empty slices) the pointer is permitted
///   to be a dangling, well-aligned non-null address. There is no payload
///   storage to reference in that case.
///
/// Aliasing discipline is enforced by wrappers that consume `InChunk`.
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
    /// Only sibling internal modules can mint `InChunk` values.
    #[inline]
    pub(super) fn from_raw(ptr: NonNull<T>) -> Self {
        Self {
            ptr,
            _phantom: PhantomData,
        }
    }

    /// Returns the underlying raw pointer.
    #[inline]
    pub(in crate::internal) fn as_ptr(self) -> *mut T {
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
    pub(in crate::internal) fn cast<U>(self) -> InChunk<U> {
        InChunk {
            ptr: self.ptr.cast(),
            _phantom: PhantomData,
        }
    }
}

impl InChunk<u8> {
    /// Returns the integer address of the pointer.
    #[inline]
    pub(in crate::internal) fn addr(self) -> usize {
        self.ptr.as_ptr() as usize
    }

    /// Builds an `InChunk<[T]>` describing `len` consecutive `T`s starting at
    /// this byte address.
    ///
    /// Caller ensures alignment for `T` and enough in-chunk storage for `len`
    /// elements.
    #[inline]
    pub(in crate::internal) fn into_slice<T>(self, len: usize) -> InChunk<[T]> {
        let slice = NonNull::slice_from_raw_parts(self.ptr.cast::<T>(), len);
        InChunk {
            ptr: slice,
            _phantom: PhantomData,
        }
    }
}
