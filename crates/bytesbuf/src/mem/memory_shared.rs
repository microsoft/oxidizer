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
/// Implementations must be [`ThreadAware`], which allows them to optimize their behavior when an
/// instance is moved between threads via a thread-aware runtime mechanism.
pub trait MemoryShared: Memory + ThreadAware + Send + Sync + 'static {
    /// Clones this provider into a boxed trait object.
    fn clone_boxed(&self) -> Box<dyn MemoryShared>;
}

impl<T> MemoryShared for T
where
    T: Memory + ThreadAware + Clone + Send + Sync + 'static,
{
    fn clone_boxed(&self) -> Box<dyn MemoryShared> {
        Box::new(self.clone())
    }
}
