// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt::Debug;

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
/// ```
///
/// The reservation logic often needs more than the wrapped provider alone. Pass a tuple (or any
/// [`ThreadAware`] value) as the data and destructure it in the callback:
///
/// ```
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
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
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
    pub fn reserve(&self, min_bytes: usize) -> BytesBuf {
        (self.reserve_fn)(&self.data, min_bytes)
    }
}

impl<D: ThreadAware + Clone + Send + Sync + 'static> Memory for CallbackMemory<D> {
    #[cfg_attr(test, mutants::skip)] // Trivial forwarder.
    fn reserve(&self, min_bytes: usize) -> BytesBuf {
        self.reserve(min_bytes)
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{self, AtomicUsize};

    use static_assertions::assert_impl_all;
    use thread_aware::affinity::{Affinity, pinned_affinities};

    use super::*;
    use crate::mem::MemoryShared;
    use crate::mem::testing::TransparentMemory;

    assert_impl_all!(CallbackMemory<TransparentMemory>: MemoryShared);

    /// Thread-aware callback data carrying an observable call counter alongside the wrapped provider.
    #[derive(Clone, Debug, ThreadAware)]
    struct CountingData {
        inner: TransparentMemory,
        // The counter is inert shared state, so relocation does not affect it.
        #[thread_aware(skip)]
        reserve_calls: Arc<AtomicUsize>,
    }

    impl CountingData {
        fn new() -> Self {
            Self {
                inner: TransparentMemory::new(),
                reserve_calls: Arc::new(AtomicUsize::new(0)),
            }
        }
    }

    // A relocation observer bundled into the data, so we can assert forwarding without a bare skip.
    #[derive(Clone, Debug)]
    struct RelocationObserver {
        inner: TransparentMemory,
        relocations: Arc<AtomicUsize>,
    }

    impl ThreadAware for RelocationObserver {
        fn relocate(&mut self, _source: Option<Affinity>, _destination: Affinity) {
            self.relocations.fetch_add(1, atomic::Ordering::SeqCst);
        }
    }

    fn count_reserve(data: &CountingData, min_bytes: usize) -> BytesBuf {
        data.reserve_calls.fetch_add(1, atomic::Ordering::SeqCst);
        data.inner.reserve(min_bytes)
    }

    #[test]
    fn calls_back_to_provided_fn() {
        let data = CountingData::new();
        let reserve_calls = Arc::clone(&data.reserve_calls);

        let provider = CallbackMemory::new(data, count_reserve);

        _ = Memory::reserve(&provider, 100);

        assert_eq!(reserve_calls.load(atomic::Ordering::SeqCst), 1);
    }

    #[test]
    fn clone_shares_behavior() {
        let data = CountingData::new();
        let reserve_calls = Arc::clone(&data.reserve_calls);

        let provider = CallbackMemory::new(data, count_reserve);
        let cloned_provider = provider.clone();

        _ = Memory::reserve(&provider, 50);
        assert_eq!(reserve_calls.load(atomic::Ordering::SeqCst), 1);

        _ = Memory::reserve(&cloned_provider, 75);
        assert_eq!(reserve_calls.load(atomic::Ordering::SeqCst), 2);
    }

    #[test]
    fn reserve_produces_capacity() {
        let provider = CallbackMemory::new(TransparentMemory::new(), TransparentMemory::reserve);

        let buf = provider.reserve(64);
        assert!(buf.capacity() >= 64);
    }

    #[test]
    fn multi_field_tuple_data() {
        // The data can carry more than the wrapped provider: here a tuple pairs it with an offset
        // that the callback adds to every reservation.
        const EXTRA: usize = 16;
        let provider = CallbackMemory::new(
            (TransparentMemory::new(), EXTRA),
            |(inner, extra): &(TransparentMemory, usize), min_bytes| inner.reserve(min_bytes + *extra),
        );

        // TransparentMemory reserves exactly the requested capacity, so the offset is observable.
        let buf = provider.reserve(64);
        assert_eq!(buf.capacity(), 64 + EXTRA);
    }

    #[test]
    fn relocate_forwards_to_data() {
        let relocations = Arc::new(AtomicUsize::new(0));
        let mut provider = CallbackMemory::new(
            RelocationObserver {
                inner: TransparentMemory::new(),
                relocations: Arc::clone(&relocations),
            },
            |observer: &RelocationObserver, min_bytes| observer.inner.reserve(min_bytes),
        );

        let affinities = pinned_affinities(&[2]);
        provider.relocate(Some(affinities[0]), affinities[1]);

        assert_eq!(relocations.load(atomic::Ordering::SeqCst), 1);

        // The provider remains usable after relocation.
        assert!(provider.reserve(32).capacity() >= 32);
    }

    #[test]
    fn works_with_non_debug_data() {
        // Data that is intentionally not `Debug`, to confirm `CallbackMemory` does not require it.
        #[derive(Clone, ThreadAware)]
        struct NotDebug {
            inner: TransparentMemory,
        }

        let provider = CallbackMemory::new(
            NotDebug {
                inner: TransparentMemory::new(),
            },
            |data: &NotDebug, min_bytes| data.inner.reserve(min_bytes),
        );

        assert!(Memory::reserve(&provider, 64).capacity() >= 64);

        // `CallbackMemory` is still `Debug` even though its data is not.
        let rendered = format!("{provider:?}");
        assert!(rendered.contains("CallbackMemory"));
    }
}
