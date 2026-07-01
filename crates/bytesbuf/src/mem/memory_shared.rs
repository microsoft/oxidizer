// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use thread_aware::ThreadAware;

use crate::mem::Memory;

/// Provides memory for byte sequences in a thread-safe manner.
///
/// This is a narrowing of [`Memory`] that adds additional constraints that enable
/// thread-safe shared access to the memory provider. If you do not need these extra
/// constraints, just use [`Memory`] directly.
///
/// # Thread awareness
///
/// A memory provider shared across threads is expected to be [thread-aware][ThreadAware], so it can
/// relocate any thread-affine state when moved between threads and avoid contention on
/// synchronization primitives. Our own providers (e.g. [`GlobalPool`][crate::mem::GlobalPool]) do
/// this internally. When implementing a memory provider, derive or implement [`ThreadAware`]; a
/// no-op implementation is correct for providers that hold no thread-affine state.
pub trait MemoryShared: Memory + ThreadAware + Send + Sync + 'static {}

impl<T> MemoryShared for T where T: Memory + ThreadAware + Send + Sync + 'static {}
