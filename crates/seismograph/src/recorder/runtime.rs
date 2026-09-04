// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Runtime event types.

use std::num::NonZeroU64;

macro_rules! identifier {
    ($name:ident, $description:literal) => {
        #[doc = $description]
        #[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
        #[repr(transparent)]
        pub struct $name(NonZeroU64);

        impl $name {
            /// Reconstructs an identifier from its stable numeric representation.
            #[must_use]
            pub const fn from_raw(value: u64) -> Option<Self> {
                match NonZeroU64::new(value) {
                    Some(value) => Some(Self(value)),
                    None => None,
                }
            }

            /// Returns the stable numeric representation.
            #[must_use]
            pub const fn get(self) -> u64 {
                self.0.get()
            }
        }
    };
}

identifier!(RuntimeId, "Process-monotonic identity assigned to a logical runtime.");
identifier!(WorkerId, "Process-monotonic identity assigned to a runtime worker.");
identifier!(TaskId, "Process-monotonic identity assigned to a runtime task.");
identifier!(TransferId, "Process-monotonic identity assigned to an instance transfer.");
identifier!(
    TypeDescriptorId,
    "Process-monotonic identity assigned to a task or instance type descriptor."
);

/// Fixed numeric context carried by a runtime event.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RuntimeEvent {
    /// Logical runtime that owns the event.
    pub runtime_id: RuntimeId,
    /// Worker associated with the event, when one exists.
    pub worker_id: Option<WorkerId>,
    /// Primary task, transfer, type, or driver identity selected by the event kind.
    pub subject_id: u64,
    /// Related task, worker, transfer, or type identity selected by the event kind.
    pub related_id: u64,
    /// First event-specific numeric value.
    pub value_0: u64,
    /// Second event-specific numeric value.
    pub value_1: u64,
}
