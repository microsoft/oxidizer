// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! This module contains all the core primitives the thread aware system is built upon.

use crate::{MemoryAffinity, PinnedAffinity};


/// Marks types that correctly handle isolation when transferred between affinities (threads).
///
/// The basic invariant of the `ThreadAware` trait is that the value returned by
/// [`ThreadAware::relocated`] must be as independent as possible from any state on the source
/// (or any other) affinity in the sense that interacting with the object should not result
/// in contention over synchronization primitives when this interaction happens in parallel
/// with interactions with related values (e.g. clones) on other affinities.
///
/// What this means depends on the type, but there are a couple of common implementation
/// strategies:
///
/// * Return self - this implies that the value doesn't have any dependency on other values
///   that may result in synchronization primitive contention, so it can be transferred as is. This
///   approach can be also be achieved by wrapping a value in the
///   [`Unaware`](`crate::Unaware`) type.
/// * Construct a per-affinity value - with this approach, each affinity gets its own
///   independently-initialized value. The [`Arc::new_with`](`crate::Arc::new_with`)
///   function facilitates this approach.
/// * Utilize true sharing in a controlled manner - have some data that is actually shared
///   between the values on different affinities, but in a controlled manner that minimizes
///   the contention for the synchronization primitives necessary. This is a more advanced
///   technique allowing for designs that minimize contention while avoiding wasting resources
///   by duplicating them for each affinity.
///
/// As an example, let's implement a counter that counts per-affinity. This counter will use
/// interior mutability to to allow increments with just a shared reference, but we want to
/// avoid contention on the internal state, so each affinity will get an independent counter.
///
/// ```rust
/// # use std::sync::atomic::{AtomicI32, Ordering};
/// # use std::sync::Arc;
/// # use thread_aware::{PinnedAffinity, ThreadAware, MemoryAffinity};
///
/// #[derive(Clone)]
/// struct Counter {
///     value: Arc<AtomicI32>,
/// }
///
/// impl Counter {
///     fn new() -> Self {
///         Self {
///             value: Arc::new(AtomicI32::new(0)),
///         }
///     }
///
///     fn increment_by(&self, value: i32) {
///         self.value.fetch_add(value, Ordering::AcqRel);
///     }
///
///     fn value(&self) -> i32 {
///         self.value.load(Ordering::Acquire)
///     }
/// }
///
/// impl ThreadAware for Counter {
///     fn relocated(self, source: MemoryAffinity, destination: PinnedAffinity) -> Self {
///         Self {
///             // Initialize a new value in the destination affinity independent
///             // of the source affinity.
///             value: Arc::new(AtomicI32::new(0)),
///         }
///     }
/// }
/// ```
///
/// Note that this trait is independent of the `Send` trait as there can be usages of isolated
/// affinities with multiple affinities on a single thread. However, that
/// is a fairly specific use case, so types that implement `ThreadAware` should generally also implement
/// Send.
pub trait ThreadAware {
    /// Consume a value and return a value in the destination affinity.
    ///
    /// When implementing this function, you can assume self belongs to the source affinity, but it's
    /// not guaranteed that source and destination will be different. Note that "belonging to an affinity"
    /// is a logical concept that may not have a direct representation in the code. In particular,
    /// when a value is first constructed, the source affinity may not be known to that value until it's
    /// transferred for the first time, at which point it can utilize the source parameter to determine
    /// the original affinity.
    ///
    /// When calling this function, you must ensure that self belongs to the source affinity, and try
    /// to avoid calling transfer when source and destination match as that's a useless operation
    /// and transfer implementations may be non-trivial.
    #[must_use]
    fn relocated(self, source: MemoryAffinity, destination: PinnedAffinity) -> Self;
}

