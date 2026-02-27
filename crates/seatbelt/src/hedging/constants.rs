// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::time::Duration;

/// Default maximum hedged attempts: 1.
///
/// One additional hedged attempt beyond the original request, resulting in 2 total
/// concurrent attempts. This provides basic speculative execution benefits while
/// limiting resource overhead.
pub(super) const DEFAULT_MAX_HEDGED_ATTEMPTS: u32 = 1;

/// Default delay between launching hedged requests: 2 seconds.
///
/// This default matches the Polly version 8 hedging default. A 2-second delay provides
/// enough time for the original request to complete in most scenarios while still
/// launching hedges quickly enough to reduce tail latency.
pub(super) const DEFAULT_HEDGING_DELAY: Duration = Duration::from_secs(2);
