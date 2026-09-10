// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Thread event types.

/// Identifier assigned to a thread-local recorder.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[repr(transparent)]
pub struct ThreadId(u64);

impl ThreadId {
    /// Creates an identifier from its stable numeric representation.
    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// Returns the stable numeric representation.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// Summary of one thread's bounded event log.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ThreadLog {
    /// Identifier assigned to the recording thread.
    pub thread_id: ThreadId,
    /// Total events emitted by the thread.
    pub total_events: u64,
    /// Events overwritten before the snapshot.
    pub lost_events: u64,
    /// Thread name, when one was assigned.
    pub name: String,
}
