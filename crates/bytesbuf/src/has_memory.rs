// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt::Debug;

use crate::MemoryShared;

/// Ability to provide memory capacity in a way suitable for shared access.
///
/// This trait is typically implemented by types that accept byte sequences or sequence builders
/// from callers. The implication is that using memory capacity obtained via this trait
/// (or [`Memory`][crate::Memory], if implemented) will allow the implementing type to perform its
/// work in the most efficient manner because the memory capacity will be pre-configured to suit the
/// specific needs of the implementing type.
///
/// # Implementation patterns
///
/// A type implementing this trait should also implement [`Memory`][crate::Memory], enabling
/// users to reserve memory capacity without needing to own an instance of the sharing-compatible
/// form of the memory provider, which may impose extra overhead.
pub trait HasMemory: Debug {
    /// Returns a sharing-compatible memory provider.
    ///
    /// The memory capacity returned by this provider will be configured to allow the implementing
    /// type to consume/produce data with optimal efficiency.
    #[must_use]
    fn memory(&self) -> impl MemoryShared;
}
