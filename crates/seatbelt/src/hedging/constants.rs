// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::time::Duration;

/// Default maximum hedged attempts: 1.
///
/// One additional hedged attempt beyond the original request, resulting in 2 total
/// concurrent attempts. This provides basic hedging benefits while
/// limiting resource overhead.
pub(super) const DEFAULT_MAX_HEDGED_ATTEMPTS: u8 = 1;

/// Default delay between launching hedged requests: 500 milliseconds.
pub(super) const DEFAULT_HEDGING_DELAY: Duration = Duration::from_millis(500);
