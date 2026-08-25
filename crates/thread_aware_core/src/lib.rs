// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![no_std]

//! Stable foundations for moving thread-isolated state between execution contexts.
//!
//! This crate contains the small API shared by thread-aware libraries:
//!
//! - [`ThreadAware`] notifies a value that it has moved to a different location.
//! - [`Location`] identifies the execution context — provenance, core and memory
//!   region — that a value has moved to.
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

mod impls;
mod location;

pub use location::{Core, Location, MemoryRegion, Provenance};

/// Marks state that can adapt after being transferred between locations.
///
/// Implementations commonly recreate location-local resources, select a
/// destination-specific handle, or do nothing when the value has no
/// location-dependent state.
///
/// This trait is a performance and contention-avoidance mechanism. A value must
/// remain correct when relocation is not reported, is reported more than once, or
/// uses equal source and destination locations.
///
/// # Example
///
/// ```
/// use thread_aware_core::{Core, Location, ThreadAware};
///
/// struct Worker {
///     core: Core,
/// }
///
/// impl ThreadAware for Worker {
///     fn relocate(&mut self, _source: Option<&Location>, destination: &Location) {
///         self.core = destination.core();
///     }
/// }
/// ```
pub trait ThreadAware: Send {
    /// Adapts this value in place for the destination location.
    ///
    /// `source` is `None` when the value's previous location is unknown. Callers
    /// should avoid unnecessary relocations, but implementations must tolerate
    /// equal source and destination locations.
    fn relocate(&mut self, source: Option<&Location>, destination: &Location);
}
