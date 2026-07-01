// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::any::type_name;
use std::fmt;
use std::sync::Arc;

use thread_aware::ThreadAware;
use thread_aware::affinity::Affinity;

use crate::BytesBuf;
use crate::mem::{Memory, MemoryShared};

/// Wraps an inner [`MemoryShared`] provider, customizing reservations via a closure.
///
/// This can be used to construct memory providers that add logic or configuration on top of an
/// existing memory provider. The wrapped provider is owned by this type (not captured by the
/// closure), so that its thread-affine state is relocated correctly when this type is moved between
/// threads: [`ThreadAware`] relocation is forwarded to the inner provider.
///
/// # Inert closures
///
/// The closure must capture only inert configuration (e.g. flags, sizes) and never any thread-affine
/// state such as another memory provider. Thread-affine state belongs in the wrapped `inner`
/// provider, which is passed to the closure as an argument, already relocated to the thread on which
/// the closure executes. Capturing thread-affine state in the closure would bypass relocation and
/// reintroduce cross-thread contention.
///
/// # Examples
///
/// Configure an inner memory provider with additional parameters:
///
/// ```
/// use bytesbuf::mem::{GlobalPool, Memory, WrappingMemory};
/// # use bytesbuf::BytesBuf;
///
/// // Wrap a pool, applying custom configuration on each reservation.
/// let memory = WrappingMemory::new(GlobalPool::new(), |pool, min_len| {
///     // Apply inert configuration when reserving memory from the (relocated) inner provider.
///     let page_aligned = true;
///     let adjusted = if page_aligned {
///         min_len.next_multiple_of(4096)
///     } else {
///         min_len
///     };
///     pool.reserve(adjusted)
/// });
///
/// let buf = memory.reserve(64);
/// assert!(buf.capacity() >= 64);
/// ```
///
/// For a complete implementation pattern, see `examples/bb_has_memory_optimizing.rs`.
pub struct WrappingMemory<T, FReserve>
where
    T: MemoryShared,
    FReserve: Fn(&T, usize) -> BytesBuf + Send + Sync + 'static,
{
    inner: T,
    reserve_fn: Arc<FReserve>,
}

impl<T, FReserve> WrappingMemory<T, FReserve>
where
    T: MemoryShared,
    FReserve: Fn(&T, usize) -> BytesBuf + Send + Sync + 'static,
{
    /// Creates a new instance wrapping `inner`, customizing reservations via `reserve_fn`.
    ///
    /// The closure receives the wrapped provider (relocated to the current thread) and the requested
    /// minimum byte count, and returns the reserved [`BytesBuf`].
    pub fn new(inner: T, reserve_fn: FReserve) -> Self {
        Self {
            inner,
            reserve_fn: Arc::new(reserve_fn),
        }
    }

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
    pub fn reserve(&self, min_bytes: usize) -> crate::BytesBuf {
        (self.reserve_fn)(&self.inner, min_bytes)
    }
}

impl<T, FReserve> Memory for WrappingMemory<T, FReserve>
where
    T: MemoryShared,
    FReserve: Fn(&T, usize) -> BytesBuf + Send + Sync + 'static,
{
    #[cfg_attr(test, mutants::skip)] // Trivial forwarder.
    fn reserve(&self, min_bytes: usize) -> BytesBuf {
        self.reserve(min_bytes)
    }
}

impl<T, FReserve> ThreadAware for WrappingMemory<T, FReserve>
where
    T: MemoryShared,
    FReserve: Fn(&T, usize) -> BytesBuf + Send + Sync + 'static,
{
    fn relocate(&mut self, source: Option<Affinity>, destination: Affinity) {
        // The wrapped provider holds any thread-affine state; the closure is inert.
        self.inner.relocate(source, destination);
    }
}

impl<T, FReserve> Clone for WrappingMemory<T, FReserve>
where
    T: MemoryShared + Clone,
    FReserve: Fn(&T, usize) -> BytesBuf + Send + Sync + 'static,
{
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            reserve_fn: Arc::clone(&self.reserve_fn),
        }
    }
}

impl<T, FReserve> fmt::Debug for WrappingMemory<T, FReserve>
where
    T: MemoryShared,
    FReserve: Fn(&T, usize) -> BytesBuf + Send + Sync + 'static,
{
    #[cfg_attr(test, mutants::skip)] // We have no API contract for this.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(type_name::<Self>())
            .field("inner", &self.inner)
            .field("reserve_fn", &"Fn(&T, usize) -> BytesBuf")
            .finish()
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use std::sync::atomic::{self, AtomicUsize};

    use static_assertions::assert_impl_all;

    use super::*;
    use crate::mem::testing::TransparentMemory;

    assert_impl_all!(
        WrappingMemory<TransparentMemory, fn(&TransparentMemory, usize) -> BytesBuf>: MemoryShared
    );

    #[test]
    fn calls_back_to_provided_fn() {
        let callback_called_times = Arc::new(AtomicUsize::new(0));

        let provider = WrappingMemory::new(TransparentMemory::new(), {
            let callback_called_times = Arc::clone(&callback_called_times);

            move |inner: &TransparentMemory, min_bytes| {
                callback_called_times.fetch_add(1, atomic::Ordering::SeqCst);
                inner.reserve(min_bytes)
            }
        });

        _ = Memory::reserve(&provider, 100);

        assert_eq!(callback_called_times.load(atomic::Ordering::SeqCst), 1);
    }

    #[test]
    fn clone_shares_underlying_callback() {
        let callback_called_times = Arc::new(AtomicUsize::new(0));

        let provider = WrappingMemory::new(TransparentMemory::new(), {
            let callback_called_times = Arc::clone(&callback_called_times);

            move |inner: &TransparentMemory, min_bytes| {
                callback_called_times.fetch_add(1, atomic::Ordering::SeqCst);
                inner.reserve(min_bytes)
            }
        });

        let cloned_provider = provider.clone();

        // Call the original provider
        _ = Memory::reserve(&provider, 50);
        assert_eq!(callback_called_times.load(atomic::Ordering::SeqCst), 1);

        // Call the cloned provider - should share the same callback
        _ = Memory::reserve(&cloned_provider, 75);
        assert_eq!(callback_called_times.load(atomic::Ordering::SeqCst), 2);
    }

    #[test]
    fn relocate_forwards_to_inner() {
        use thread_aware::affinity::pinned_affinities;

        // A provider whose relocate is observable, to verify forwarding.
        #[derive(Clone, Debug)]
        struct TrackingMemory {
            relocated: Arc<AtomicUsize>,
            inner: TransparentMemory,
        }

        impl Memory for TrackingMemory {
            fn reserve(&self, min_bytes: usize) -> BytesBuf {
                self.inner.reserve(min_bytes)
            }
        }

        impl ThreadAware for TrackingMemory {
            fn relocate(&mut self, _source: Option<Affinity>, _destination: Affinity) {
                self.relocated.fetch_add(1, atomic::Ordering::SeqCst);
            }
        }

        let relocated = Arc::new(AtomicUsize::new(0));
        let mut provider = WrappingMemory::new(
            TrackingMemory {
                relocated: Arc::clone(&relocated),
                inner: TransparentMemory::new(),
            },
            move |inner: &TrackingMemory, min_bytes| inner.reserve(min_bytes),
        );

        let affinities = pinned_affinities(&[2]);
        provider.relocate(Some(affinities[0]), affinities[1]);

        assert_eq!(relocated.load(atomic::Ordering::SeqCst), 1);
    }

    #[test]
    fn debug_output_contains_type_and_field_info() {
        let provider = WrappingMemory::new(TransparentMemory::new(), |inner: &TransparentMemory, min_bytes| {
            inner.reserve(min_bytes)
        });

        // Call the original provider to help code coverage.
        _ = Memory::reserve(&provider, 50);

        let debug_output = format!("{provider:?}");

        // Verify the debug output contains the struct name and field description
        assert!(debug_output.contains("WrappingMemory"), "Debug output should contain type name");
        assert!(debug_output.contains("reserve_fn"), "Debug output should contain field name");
        assert!(
            debug_output.contains("Fn(&T, usize) -> BytesBuf"),
            "Debug output should contain function signature description"
        );
    }
}
