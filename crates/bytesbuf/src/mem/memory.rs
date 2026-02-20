// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt::Debug;

use crate::BytesBuf;

/// Provides memory capacity for byte sequences.
///
/// Call [`reserve()`][Self::reserve] to reserve memory capacity and obtain a [`BytesBuf`]
/// that can be used to fill the reserved memory with data.
#[doc = include_str!("../../doc/snippets/choosing_memory_provider.md")]
///
/// # Resource management
///
/// The reserved memory is released when the last [`BytesBuf`] or
/// [`BytesView`][crate::BytesView] referencing it is dropped.
pub trait Memory: Debug {
    /// Reserves at least `min_bytes` bytes of memory capacity.
    ///
    /// Returns an empty [`BytesBuf`] that can be used to fill the reserved memory with data.
    ///
    /// The memory provider may provide more memory than requested.
    ///
    /// The memory reservation request will always be fulfilled, obtaining more memory from the
    /// operating system if necessary.
    ///
    /// # Zero-sized reservations
    ///
    /// Reserving zero bytes of memory is a valid operation and will return a [`BytesBuf`]
    /// with zero or more bytes of capacity.
    ///
    /// # Panics
    ///
    /// May panic if the operating system runs out of memory.
    #[must_use]
    fn reserve(&self, min_bytes: usize) -> BytesBuf;
}

impl<M: Memory + ?Sized> Memory for &M {
    #[inline]
    fn reserve(&self, min_bytes: usize) -> BytesBuf {
        (*self).reserve(min_bytes)
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::Memory;
    use crate::BytesBuf;
    use crate::mem::testing::TransparentMemory;

    #[derive(Debug)]
    struct CountingMemory {
        reserve_calls: AtomicUsize,
        inner: TransparentMemory,
    }

    impl CountingMemory {
        fn new() -> Self {
            Self {
                reserve_calls: AtomicUsize::new(0),
                inner: TransparentMemory::new(),
            }
        }
    }

    impl Memory for CountingMemory {
        fn reserve(&self, min_bytes: usize) -> BytesBuf {
            self.reserve_calls.fetch_add(1, Ordering::SeqCst);
            self.inner.reserve(min_bytes + 16)
        }
    }

    fn reserve_from_generic<M: Memory>(memory: M, min_bytes: usize) -> BytesBuf {
        memory.reserve(min_bytes)
    }

    #[test]
    fn memory_impl_for_reference_forwards_reserve_to_underlying() {
        let memory = CountingMemory::new();
        let min_bytes = 64;

        let buf = reserve_from_generic(&memory, min_bytes);

        assert_eq!(memory.reserve_calls.load(Ordering::SeqCst), 1);
        assert!(buf.capacity() >= min_bytes + 16);
    }
}
