// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

/// A type-erased, cloneable, thread-safe memory provider.
///
/// Uses an enum to avoid `Arc` allocation and dynamic dispatch for the
/// common [`GlobalPool`] case, falling back to `Arc<dyn Fn>` for custom
/// memory providers.
use std::fmt;
use std::sync::Arc;

use bytesbuf::BytesBuf;
use bytesbuf::mem::{GlobalPool, Memory, MemoryShared};

#[derive(Clone)]
pub struct SharedMemory {
    inner: SharedMemoryInner,
}

#[derive(Clone)]
enum SharedMemoryInner {
    Global(GlobalPool),
    Custom(Arc<dyn Fn(usize) -> BytesBuf + Send + Sync>),
}

impl SharedMemory {
    /// Creates a `SharedMemory` from any `MemoryShared` implementation.
    pub fn new(memory: impl MemoryShared) -> Self {
        Self {
            inner: SharedMemoryInner::Custom(Arc::new(move |min_bytes| memory.reserve(min_bytes))),
        }
    }

    /// Creates a `SharedMemory` backed by the default [`GlobalPool`].
    pub fn global() -> Self {
        Self {
            inner: SharedMemoryInner::Global(GlobalPool::new()),
        }
    }
}

impl fmt::Debug for SharedMemory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SharedMemory").finish_non_exhaustive()
    }
}

impl Memory for SharedMemory {
    fn reserve(&self, min_bytes: usize) -> BytesBuf {
        match &self.inner {
            SharedMemoryInner::Global(pool) => pool.reserve(min_bytes),
            SharedMemoryInner::Custom(f) => f(min_bytes),
        }
    }
}
