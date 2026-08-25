// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use core::error::Error;
use core::fmt;

/// Why a [`Pool`](crate::Pool) allocation failed.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
enum ErrorKind {
    /// A configured capacity limit was reached: a pool's chunk-growth limit
    /// (the `max_chunks` cap, or, for an unbounded pool, the addressable
    /// slot-index ceiling), or a multi pool's `max_layouts` limit on the number
    /// of internal layout pools.
    CapacityExhausted,
    /// The pool could not obtain memory needed for its own use.
    AllocatorFailed,
}

/// The error returned by the fallible `try_alloc_*` methods of
/// [`Pool`](crate::Pool) and [`MultiPool`](crate::MultiPool).
///
/// Distinguish the two causes with
/// [`is_capacity_exhausted`](Self::is_capacity_exhausted) and
/// [`is_allocator_failure`](Self::is_allocator_failure).
///
/// In both cases the rejected value is dropped and any `_with` closure is left
/// uncalled.
///
/// Like [`core::alloc::AllocError`], this carries no backtrace or source error.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub struct AllocError {
    kind: ErrorKind,
}

impl AllocError {
    /// The pool reached its capacity limit (see [`is_capacity_exhausted`]).
    ///
    /// [`is_capacity_exhausted`]: Self::is_capacity_exhausted
    pub(crate) const CAPACITY_EXHAUSTED: Self = Self {
        kind: ErrorKind::CapacityExhausted,
    };

    /// The pool could not obtain required memory (see
    /// [`is_allocator_failure`]).
    ///
    /// [`is_allocator_failure`]: Self::is_allocator_failure
    pub(crate) const ALLOCATOR_FAILED: Self = Self {
        kind: ErrorKind::AllocatorFailed,
    };

    /// Returns `true` if a configured capacity limit was reached: either the
    /// pool may grow no further, or a [`MultiPool`](crate::MultiPool) reached
    /// its limit on the number of internal layout pools and the request needed
    /// a new one.
    #[must_use]
    pub fn is_capacity_exhausted(self) -> bool {
        matches!(self.kind, ErrorKind::CapacityExhausted)
    }

    /// Returns `true` if the pool could not obtain memory needed for its own
    /// use.
    #[must_use]
    pub fn is_allocator_failure(self) -> bool {
        matches!(self.kind, ErrorKind::AllocatorFailed)
    }
}

impl fmt::Display for AllocError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self.kind {
            ErrorKind::CapacityExhausted => "the pool reached its maximum capacity",
            ErrorKind::AllocatorFailed => "the pool could not obtain required memory",
        })
    }
}

impl Error for AllocError {}
