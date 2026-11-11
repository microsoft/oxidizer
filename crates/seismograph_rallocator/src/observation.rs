// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Instant;

static PUBLICATION_ENABLED: AtomicBool = AtomicBool::new(true);
static OBSERVATION_ROUND: AtomicU64 = AtomicU64::new(1);
static EPOCH: OnceLock<Instant> = OnceLock::new();

/// Whether operations with allocation recording enabled can publish owner copies.
#[must_use]
pub fn publication_enabled() -> bool {
    PUBLICATION_ENABLED.load(Ordering::Relaxed)
}

/// Changes owner publication for both backends without enabling event recording.
pub fn set_publication_enabled(enabled: bool) {
    PUBLICATION_ENABLED.store(enabled, Ordering::Relaxed);
}

/// Round requested by the most recent collector or explicit application request.
#[must_use]
pub fn observation_round() -> u64 {
    OBSERVATION_ROUND.load(Ordering::Relaxed)
}

/// Requests another copy after an operation with allocation recording enabled.
///
/// Collectors request a new round after capture. No timer or background thread
/// is started, and quiet owners can retain older observations.
pub fn request_observation() -> u64 {
    let previous = OBSERVATION_ROUND.try_update(Ordering::Relaxed, Ordering::Relaxed, |round| Some(round.saturating_add(1)));
    match previous {
        Ok(round) | Err(round) => round.saturating_add(1),
    }
}

/// Process-relative monotonic capture time, saturated to the wire representation.
#[must_use]
pub fn captured_nanos() -> u64 {
    u64::try_from(EPOCH.get_or_init(Instant::now).elapsed().as_nanos()).unwrap_or(u64::MAX)
}
