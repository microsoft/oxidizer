// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

/// The name of the retry event for telemetry reporting.
pub(super) const RETRY_EVENT: &str = "retry";

/// Attribute key for the retry attempt index.
pub(super) const ATTEMPT_INDEX: &str = "resilience.attempt.index";

/// Attribute key for whether this is the last retry attempt.
pub(super) const ATTEMPT_NUMBER_IS_LAST: &str = "resilience.attempt.is_last";
