// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

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
    captured_nanos_from_elapsed(native_elapsed().as_nanos())
}

fn native_elapsed() -> Duration {
    #[cfg(test)]
    if let Some(nanos) = TEST_NATIVE_NANOS.get() {
        return Duration::new(
            u64::try_from(nanos / 1_000_000_000).unwrap(),
            u32::try_from(nanos % 1_000_000_000).unwrap(),
        );
    }
    EPOCH.get_or_init(Instant::now).elapsed()
}

fn captured_nanos_from_elapsed(nanos: u128) -> u64 {
    u64::try_from(nanos).unwrap_or(u64::MAX)
}

#[cfg(test)]
thread_local! {
    static TEST_NATIVE_NANOS: std::cell::Cell<Option<u128>> = const { std::cell::Cell::new(None) };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_timestamp_preserves_exact_values_and_saturation_without_observing_a_clock() {
        struct Restore(Option<u128>);

        impl Drop for Restore {
            fn drop(&mut self) {
                TEST_NATIVE_NANOS.set(self.0);
            }
        }

        let _restore = Restore(TEST_NATIVE_NANOS.get());
        for (input, expected) in [
            (0, 0),
            (37, 37),
            (u128::from(u64::MAX), u64::MAX),
            (u128::from(u64::MAX) + 1, u64::MAX),
        ] {
            TEST_NATIVE_NANOS.set(Some(input));
            assert_eq!(captured_nanos(), expected);
            assert_eq!(captured_nanos_from_elapsed(input), expected);
        }
    }
}
