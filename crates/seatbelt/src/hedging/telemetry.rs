// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

/// The name of the hedge event for telemetry reporting.
pub(super) const HEDGE_EVENT: &str = "hedge";

/// Attribute key for the hedge attempt index.
pub(super) const ATTEMPT_INDEX: &str = "resilience.attempt.index";

/// Attribute key indicating whether this is the last hedge attempt.
pub(super) const ATTEMPT_IS_LAST: &str = "resilience.attempt.is_last";
