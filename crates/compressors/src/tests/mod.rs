// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! White-box behaviour tests for the crate's cross-cutting contracts.
//!
//! These drive concrete engines through the crate-private push/pull mechanics and inspect the
//! private step outcomes, which is what lets them assert the state machine's transitions rather
//! than only its end results. That access is why they live inside the crate: a separate test crate
//! cannot name those items, and exposing them so it could would defeat the sealing they exist to
//! verify.

#[cfg(any(test, any_format))]
mod format_contract;

#[cfg(any(test, feature = "gzip"))]
mod round_trip;
