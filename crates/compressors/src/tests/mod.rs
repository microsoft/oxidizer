// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! White-box behavior tests for the crate contracts that span modules.
//!
//! These drive concrete engines through the crate-private push/pull mechanics and inspect the
//! private step outcomes, which is what lets them assert the state machine's transitions rather
//! than only its end results. That access is why they live inside the crate: a separate test crate
//! cannot name those items. Exposing them defeats the sealing that they
//! test.

#[cfg(any(test, any_format))]
mod format_contract;

#[cfg(any(test, feature = "gzip"))]
mod round_trip;
