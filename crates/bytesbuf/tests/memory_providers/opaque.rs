// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Tests behavior through the public API.

use std::sync::Arc;
use std::sync::atomic::{self, AtomicUsize};

use bytesbuf::mem::{GlobalPool, Memory, MemoryShared, OpaqueMemory};
use static_assertions::assert_impl_all;
use thread_aware::{Relocator, Thread, ThreadAware};

assert_impl_all!(OpaqueMemory: MemoryShared, ThreadAware);

#[test]
fn wraps_inner() {
    let provider = GlobalPool::new();
    let memory = OpaqueMemory::new(provider);
    let cloned = memory.clone();

    assert!(memory.reserve(1024).capacity() >= 1024);
    assert!(cloned.reserve(1024).capacity() >= 1024);
}

#[test]
fn memory_trait() {
    let provider = GlobalPool::new();
    let memory = OpaqueMemory::new(provider);

    // Call reserve via the Memory trait to verify the impl block
    let builder = Memory::reserve(&memory, 1024);
    assert!(builder.capacity() >= 1024);
}

#[test]
fn relocate_forwards_to_wrapped_provider() {
    // A provider whose relocate is observable, to verify forwarding.
    #[derive(Clone, Debug)]
    struct TrackingMemory {
        relocated: Arc<AtomicUsize>,
        inner: GlobalPool,
    }

    impl Memory for TrackingMemory {
        fn reserve(&self, min_bytes: usize) -> bytesbuf::BytesBuf {
            self.inner.reserve(min_bytes)
        }
    }

    impl ThreadAware for TrackingMemory {
        fn relocate(&mut self, source: Option<&Thread>, destination: &Thread) {
            self.relocated.fetch_add(1, atomic::Ordering::SeqCst);
            self.inner.relocate(source, destination);
        }
    }

    let relocated = Arc::new(AtomicUsize::new(0));
    let mut memory = OpaqueMemory::new(TrackingMemory {
        relocated: Arc::clone(&relocated),
        inner: GlobalPool::new(),
    });

    _ = Relocator::between_threads().relocate(&mut memory);

    assert_eq!(relocated.load(atomic::Ordering::SeqCst), 1);
}
