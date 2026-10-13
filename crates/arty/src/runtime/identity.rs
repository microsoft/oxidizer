// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt;
use std::num::NonZeroU64;

/// Stable process-local identity for one Arty runtime.
///
/// The numeric value correlates this runtime with its entry in a decoded
/// `seismograph_runtime` snapshot without exposing Seismograph types in Arty's
/// public API. Identities are unique within one process execution.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct RuntimeId(NonZeroU64);

impl RuntimeId {
    pub(crate) fn from_seismograph(value: seismograph::recorder::runtime::RuntimeId) -> Self {
        Self(NonZeroU64::new(value.get()).expect("Seismograph runtime identities are nonzero"))
    }

    /// Returns the process-local numeric identity.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0.get()
    }
}

impl fmt::Display for RuntimeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}
