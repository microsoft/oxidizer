// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::any::type_name;
use std::fmt;
use std::sync::Arc;

use crate::BytesBuf;
use crate::mem::Memory;

/// Implements [`MemoryShared`][crate::mem::MemoryShared] by delegating to a closure.
///
/// This can be used to construct wrapping memory providers that add logic or configuration
/// on top of an existing memory provider.
pub struct CallbackMemory<FReserve>
where
    FReserve: Fn(usize) -> BytesBuf + Send + Sync + 'static,
{
    reserve_fn: Arc<FReserve>,
}

impl<FReserve> CallbackMemory<FReserve>
where
    FReserve: Fn(usize) -> BytesBuf + Send + Sync + 'static,
{
    /// Creates a new instance implemented via the provided callback.
    pub fn new(reserve_fn: FReserve) -> Self {
        Self {
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
        (self.reserve_fn)(min_bytes)
    }
}

impl<FReserve> Memory for CallbackMemory<FReserve>
where
    FReserve: Fn(usize) -> BytesBuf + Send + Sync + 'static,
{
    #[cfg_attr(test, mutants::skip)] // Trivial forwarder.
    fn reserve(&self, min_bytes: usize) -> BytesBuf {
        self.reserve(min_bytes)
    }
}

impl<FReserve> Clone for CallbackMemory<FReserve>
where
    FReserve: Fn(usize) -> BytesBuf + Send + Sync + 'static,
{
    fn clone(&self) -> Self {
        Self {
            reserve_fn: Arc::clone(&self.reserve_fn),
        }
    }
}

impl<FReserve> fmt::Debug for CallbackMemory<FReserve>
where
    FReserve: Fn(usize) -> BytesBuf + Send + Sync + 'static,
{
    #[cfg_attr(test, mutants::skip)] // We have no API contract for this.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(type_name::<Self>())
            .field("reserve_fn", &"Fn(usize) -> BytesBuf")
            .finish()
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use std::sync::atomic::{self, AtomicUsize};

    use static_assertions::assert_impl_all;

    use super::*;
    use crate::mem::MemoryShared;
    use crate::mem::testing::TransparentMemory;

    assert_impl_all!(CallbackMemory<fn(usize) -> BytesBuf>: MemoryShared);

    #[test]
    fn calls_back_to_provided_fn() {
        let callback_called_times = Arc::new(AtomicUsize::new(0));

        let provider = CallbackMemory::new({
            let callback_called_times = Arc::clone(&callback_called_times);

            move |min_bytes| {
                callback_called_times.fetch_add(1, atomic::Ordering::SeqCst);
                TransparentMemory::new().reserve(min_bytes)
            }
        });

        _ = Memory::reserve(&provider, 100);

        assert_eq!(callback_called_times.load(atomic::Ordering::SeqCst), 1);
    }

    #[test]
    fn clone_shares_underlying_callback() {
        let callback_called_times = Arc::new(AtomicUsize::new(0));

        let provider = CallbackMemory::new({
            let callback_called_times = Arc::clone(&callback_called_times);

            move |min_bytes| {
                callback_called_times.fetch_add(1, atomic::Ordering::SeqCst);
                TransparentMemory::new().reserve(min_bytes)
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
    fn debug_output_contains_type_and_field_info() {
        let provider = CallbackMemory::new(|min_bytes| TransparentMemory::new().reserve(min_bytes));

        // Call the original provider to help code coverage.
        _ = Memory::reserve(&provider, 50);

        let debug_output = format!("{provider:?}");

        // Verify the debug output contains the struct name and field description
        assert!(debug_output.contains("CallbackMemory"), "Debug output should contain type name");
        assert!(debug_output.contains("reserve_fn"), "Debug output should contain field name");
        assert!(
            debug_output.contains("Fn(usize) -> BytesBuf"),
            "Debug output should contain function signature description"
        );
    }
}
