// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use core::fmt::Debug;

use crate::mem::MemoryShared;

/// Ability to provide memory capacity in a way suitable for shared access.
///
/// This trait is typically implemented by types that accept byte sequences or sequence builders
/// from callers. The implication is that using memory capacity obtained via this trait
/// (or [`Memory`][crate::mem::Memory], if implemented) will allow the implementing type to perform its
/// work in the most efficient manner because the memory capacity will be pre-configured to suit the
/// specific needs of the implementing type.
///
/// # Implementation patterns
///
/// A type implementing this trait should also implement [`Memory`][crate::mem::Memory], enabling
/// users to reserve memory capacity without needing to own an instance of the sharing-compatible
/// form of the memory provider, which may impose extra overhead.
///
/// An I/O endpoint usually returns a provider supplied or configured by its I/O implementation.
/// A wrapper forwards the provider of the object it wraps. A usage-neutral library that needs no
/// special configuration stores a provider supplied by the application, usually as
/// [`OpaqueMemory`][crate::mem::OpaqueMemory], and returns clones of it. Such a library does not
/// create its own `GlobalPool`.
pub trait HasMemory: Debug {
    /// Returns a sharing-compatible memory provider.
    ///
    /// The memory capacity returned by this provider will be configured to allow the implementing
    /// type to consume/produce data with optimal efficiency.
    #[must_use]
    fn memory(&self) -> impl MemoryShared;
}
