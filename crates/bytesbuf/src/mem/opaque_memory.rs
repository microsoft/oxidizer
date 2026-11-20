// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#[cfg(not(test))]
use alloc::boxed::Box;

use thread_aware::ThreadAware;

use crate::mem::{Memory, MemoryShared};

/// Adapter to erase the type of a [`MemoryShared`] implementation.
///
/// This adapter adds some inefficiency due to additional indirection overhead for
/// every memory reservation, so avoid this adapter if you can tolerate alternatives (generics).
///
/// The adapter is itself [`MemoryShared`]. It owns the wrapped provider and forwards [`ThreadAware`]
/// relocation to it, leaving the decision of how to be thread-aware entirely with the wrapped
/// provider. Cloning the adapter clones the wrapped provider; whether the clones then share any
/// state is up to that provider.
///
/// With the `std` feature, `GlobalPool` converts into this adapter through [`Into::into`].
#[derive(Debug, ThreadAware)]
pub struct OpaqueMemory {
    inner: Box<dyn MemoryShared>,
}

impl OpaqueMemory {
    /// Creates a new instance of the adapter.
    #[must_use]
    pub fn new(inner: impl MemoryShared) -> Self {
        Self { inner: Box::new(inner) }
    }

    /// Reserves at least `min_bytes` bytes of memory capacity.
    ///
    /// Returns an empty [`BytesBuf`][1] that can be used to fill the reserved memory with data.
    ///
    /// The memory provider may provide more memory than requested.
    ///
    /// If this method returns, the buffer has at least `min_bytes` bytes of capacity.
    ///
    /// # Zero-sized reservations
    ///
    /// Reserving zero bytes of memory is a valid operation and will return a [`BytesBuf`][1]
    /// with zero or more bytes of capacity.
    ///
    /// # Panics
    ///
    /// The wrapped provider may panic or abort if the requested capacity cannot be obtained.
    ///
    /// [1]: crate::BytesBuf
    #[must_use]
    pub fn reserve(&self, min_bytes: usize) -> crate::BytesBuf {
        self.inner.reserve(min_bytes)
    }
}

impl Clone for OpaqueMemory {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone_boxed(),
        }
    }
}

impl Memory for OpaqueMemory {
    fn reserve(&self, min_bytes: usize) -> crate::BytesBuf {
        self.reserve(min_bytes)
    }
}
