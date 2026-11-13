// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use core::fmt::Debug;

use thread_aware::ThreadAware;

use crate::BytesBuf;
use crate::mem::Memory;

/// A [`MemoryShared`][crate::mem::MemoryShared] provider that customizes reservations via a function.
///
/// This can be used to construct memory providers that add logic or configuration on top of an
/// existing memory provider, without implementing a dedicated provider type.
///
/// Modeled on [`thread_aware::closure::Closure`]: because `reserve_fn` is a bare `fn` pointer it
/// cannot capture anything, so all state the reservation logic needs must live in `data`. As `data`
/// is [`ThreadAware`], it is relocated together with the provider when the provider is moved between
/// threads via a thread-aware runtime mechanism.
///
/// # Examples
///
/// Wrap an existing memory provider, page-aligning every reservation. The wrapped provider is the
/// callback's thread-aware data; the reservation logic is a bare function that cannot capture
/// anything.
///
/// ```
/// # fn main() {
/// # #[cfg(feature = "std")] {
/// use bytesbuf::mem::{CallbackMemory, Memory};
/// # use bytesbuf::BytesBuf;
/// # use bytesbuf::mem::GlobalPool;
/// # let inner = GlobalPool::new();
///
/// let memory = CallbackMemory::new(inner, |inner, min_len| {
///     inner.reserve(min_len.next_multiple_of(4096))
/// });
///
/// let buf = memory.reserve(64);
/// assert!(buf.capacity() >= 64);
/// # }
/// # }
/// ```
///
/// The reservation logic often needs more than the wrapped provider alone. Pass a tuple (or any
/// [`ThreadAware`] value) as the data and destructure it in the callback:
///
/// ```
/// # fn main() {
/// # #[cfg(feature = "std")] {
/// use bytesbuf::mem::{CallbackMemory, Memory};
/// # use bytesbuf::BytesBuf;
/// # use bytesbuf::mem::GlobalPool;
/// # let inner = GlobalPool::new();
///
/// let alignment = 4096_usize;
/// let memory = CallbackMemory::new((inner, alignment), |(inner, alignment), min_len| {
///     inner.reserve(min_len.next_multiple_of(*alignment))
/// });
///
/// let buf = memory.reserve(64);
/// assert!(buf.capacity() >= 64);
/// # }
/// # }
/// ```
///
/// For a complete implementation pattern, see `examples/bb_has_memory_optimizing.rs`.
#[derive(Clone, ThreadAware)]
pub struct CallbackMemory<D: ThreadAware + Clone + Send + Sync + 'static> {
    data: D,
    // The function pointer holds no state; only the captured data can be thread-affine.
    #[thread_aware(skip)]
    reserve_fn: fn(&D, usize) -> BytesBuf,
}

impl<D: ThreadAware + Clone + Send + Sync + 'static> Debug for CallbackMemory<D> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        // `data` is deliberately not formatted so that `CallbackMemory` is `Debug` regardless of
        // whether `D` is, which keeps it usable with thread-aware data that is intentionally not
        // `Debug` (e.g. a tuple containing a non-`Debug` field).
        f.debug_struct("CallbackMemory")
            .field("reserve_fn", &self.reserve_fn)
            .finish_non_exhaustive()
    }
}

impl<D: ThreadAware + Clone + Send + Sync + 'static> CallbackMemory<D> {
    /// Creates a provider that reserves memory via `reserve_fn`, applied to thread-aware `data`.
    ///
    /// `data` holds any state the reservation needs (typically the wrapped memory provider). Because
    /// `reserve_fn` is a bare `fn` pointer it cannot capture anything, so all such state must live in
    /// `data`, which is relocated with the provider when it is moved between threads via a
    /// thread-aware runtime mechanism.
    #[must_use]
    pub fn new(data: D, reserve_fn: fn(&D, usize) -> BytesBuf) -> Self {
        Self { data, reserve_fn }
    }

    /// Reserves at least `min_bytes` bytes of memory capacity.
    ///
    /// Returns an empty [`BytesBuf`] that can be used to fill the reserved memory with data.
    ///
    /// The memory provider may provide more memory than requested.
    ///
    /// If this method returns, the buffer has at least `min_bytes` bytes of capacity.
    ///
    /// # Zero-sized reservations
    ///
    /// Reserving zero bytes of memory is a valid operation and will return a [`BytesBuf`]
    /// with zero or more bytes of capacity.
    ///
    /// # Panics
    ///
    /// The callback may panic or abort if the requested capacity cannot be obtained.
    #[must_use]
    pub fn reserve(&self, min_bytes: usize) -> BytesBuf {
        (self.reserve_fn)(&self.data, min_bytes)
    }
}

impl<D: ThreadAware + Clone + Send + Sync + 'static> Memory for CallbackMemory<D> {
    fn reserve(&self, min_bytes: usize) -> BytesBuf {
        self.reserve(min_bytes)
    }
}
