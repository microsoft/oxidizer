// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::time::Duration;

/// Sets the default maximum to one hedged attempt.
///
/// This permits two concurrent attempts in total.
pub(super) const DEFAULT_MAX_HEDGED_ATTEMPTS: u8 = 1;

/// Sets the default delay between hedged attempts to 500 milliseconds.
pub(super) const DEFAULT_HEDGING_DELAY: Duration = Duration::from_millis(500);
