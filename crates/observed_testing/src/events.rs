// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Reusable event definitions shared across integration tests.
//!
//! Only truly shared events live here. Events used by a single test file
//! are defined locally in that test file.

use observed::event;

use crate::types::PublicI64;

/// A simple event with a single `i64` field, useful as a minimal probe.
#[event("test.probe")]
#[info]
#[derive(Debug)]
pub struct ProbeEvent {
    /// The probe's single classified `i64` payload.
    pub value: PublicI64,
}

impl ProbeEvent {
    /// Creates a new [`ProbeEvent`] carrying `value`.
    #[must_use]
    pub fn new(value: i64) -> Self {
        Self { value: PublicI64(value) }
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_event_exposes_type_id() {
        // Compile-time derived events carry a `TypeId`; this guards
        // `ProbeEvent::DESCRIPTION.type_id()` against regressing to `None`.
        assert_eq!(
            <ProbeEvent as observed::Event>::DESCRIPTION.type_id(),
            Some(std::any::TypeId::of::<ProbeEvent>()),
        );
    }
}
