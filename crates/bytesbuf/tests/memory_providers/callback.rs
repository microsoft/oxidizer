// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Tests behavior through the public API.

use std::sync::Arc;
use std::sync::atomic::{self, AtomicUsize};

use bytesbuf::BytesBuf;
use bytesbuf::mem::testing::TransparentMemory;
use bytesbuf::mem::{CallbackMemory, Memory, MemoryShared};
use static_assertions::assert_impl_all;
use thread_aware::{Relocator, Thread, ThreadAware};

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
    fn relocate(&mut self, _source: Option<&Thread>, _destination: &Thread) {
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

    _ = Relocator::between_threads().relocate(&mut provider);

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
