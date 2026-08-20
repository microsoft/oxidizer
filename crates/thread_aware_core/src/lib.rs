// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![no_std]

//! Stable foundations for moving thread-isolated state between execution contexts.
//!
//! This crate contains the small API shared by thread-aware libraries:
//!
//! - [`ThreadAware`] notifies a value that it has moved to a different affinity.
//! - [`Affinity`] identifies the processor and memory region associated with an
//!   execution context.
//! - [`pinned_affinities`] creates affinity identifiers for a known topology.
//!
//! Relocation is a cooperative performance optimization rather than a correctness
//! boundary. Implementations must remain correct if a relocation notification is
//! omitted, repeated, or reports the same source and destination.
//!
//! The crate has no dependencies and is always `no_std`. Its opt-in `std`
//! feature adds implementations for standard-library types such as `HashMap`
//! and `Path`.

extern crate alloc;
#[cfg(any(feature = "std", test))]
extern crate std;

mod affinity;
pub mod affinity2;
pub mod affinity3;
mod impls;

pub use affinity::{Affinity, pinned_affinities};

/// Marks state that can adapt after being transferred between affinities.
///
/// Implementations commonly recreate affinity-local resources, select a
/// destination-specific handle, or do nothing when the value has no
/// affinity-dependent state.
///
/// This trait is a performance and contention-avoidance mechanism. A value must
/// remain correct when relocation is not reported, is reported more than once, or
/// uses equal source and destination affinities.
///
/// # Example
///
/// ```
/// use thread_aware_core::{Affinity, ThreadAware};
///
/// struct Worker {
///     processor_index: usize,
/// }
///
/// impl ThreadAware for Worker {
///     fn relocate(&mut self, _source: Option<Affinity>, destination: Affinity) {
///         self.processor_index = destination.processor_index();
///     }
/// }
/// ```
pub trait ThreadAware: Send {
    /// Adapts this value in place for the destination affinity.
    ///
    /// `source` is `None` when the value's previous affinity is unknown. Callers
    /// should avoid unnecessary relocations, but implementations must tolerate
    /// equal source and destination affinities.
    fn relocate(&mut self, source: Option<Affinity>, destination: Affinity);
}
