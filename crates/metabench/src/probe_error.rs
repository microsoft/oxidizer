// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::num::ParseIntError;

use crate::error::Error;

#[ohno::error]
#[display("expected private native probe worker arguments")]
pub(crate) struct InvalidProbeArguments;

#[ohno::error]
#[display("probe {level} index is out of bounds")]
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "retain structured indices without changing existing worker diagnostics")
)]
pub(crate) struct ProbeIndexOutOfBounds {
    pub(crate) level: &'static str,
    pub(crate) index: usize,
    pub(crate) count: usize,
}

#[ohno::error]
#[display("probe iterator selection does not match the native case")]
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "retain structured selection details without changing existing worker diagnostics"
    )
)]
pub(crate) struct ProbeSelectionMismatch {
    pub(crate) iterator: bool,
    pub(crate) iteration: Option<usize>,
}

#[ohno::error]
#[display("native probe case count exceeds {limit}")]
pub(crate) struct ProbeCaseLimit {
    pub(crate) limit: usize,
}

#[ohno::error]
#[display("duplicate native probe identity {identity:?}; rename the conflicting case or benchmark")]
pub(crate) struct DuplicateProbeIdentity {
    pub(crate) identity: String,
}

#[ohno::error]
#[display("duplicate native probe backend identity {identity:?}; rename the conflicting case or benchmark")]
pub(crate) struct DuplicateProbeBackendIdentity {
    pub(crate) identity: String,
}

#[ohno::error]
#[display("missing native probe discovery artifact path")]
pub(crate) struct MissingProbeDiscoveryPath;

#[ohno::error]
#[from(InvalidProbeArguments)]
#[from(ProbeIndexOutOfBounds)]
#[from(ProbeSelectionMismatch)]
#[from(ProbeCaseLimit)]
#[from(DuplicateProbeIdentity)]
#[from(DuplicateProbeBackendIdentity)]
#[from(MissingProbeDiscoveryPath)]
#[from(ParseIntError)]
#[from(Error)]
pub(crate) struct ProbeError;
