// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::time::{Duration, SystemTime};

use http::header::RETRY_AFTER;
use http::{HeaderMap, Response};
use recoverable::{Attempt, RecoveryInfo, RecoveryKind};
use tick::Clock;
use tick::fmt::Rfc2822;

use crate::{HeaderMapExt, StatusExt};

/// Response recovery classification with `Retry-After` support.
///
/// Extends recovery classification to consider the `Retry-After` header.
pub trait ResponseExt: sealed::Sealed {
    /// Returns recovery classification of the response, considering `Retry-After` header.
    fn recovery_with_clock(&self, clock: &Clock) -> RecoveryInfo;

    /// Returns the [`Attempt`] recorded on this response, if any.
    ///
    /// Resilience middleware can attach the [`Attempt`] that produced a response
    /// so downstream consumers can correlate the response with the attempt.
    /// Returns `None` when no attempt has been recorded.
    fn attempt(&self) -> Option<Attempt>;

    /// Records the [`Attempt`] that produced this response.
    ///
    /// Replaces any attempt previously attached to the response's extensions.
    fn set_attempt(&mut self, attempt: Attempt);
}

impl<B> ResponseExt for Response<B> {
    /// Returns recovery classification of the response.
    ///
    /// In addition to the [standard recovery classification][StatusExt::recovery], based on status code,
    /// this method also considers the `Retry-After` header for `Retry` recoveries.
    ///
    /// For time manipulation, the provided `Clock` is used.
    fn recovery_with_clock(&self, clock: &Clock) -> RecoveryInfo {
        let recovery = self.recovery();

        match recovery.kind() {
            RecoveryKind::Retry => {
                get_retry_after_duration(self.headers(), clock).map_or_else(|| recovery, |d| RecoveryInfo::retry().delay(d))
            }
            _ => recovery,
        }
    }

    fn attempt(&self) -> Option<Attempt> {
        self.extensions().get::<Attempt>().copied()
    }

    fn set_attempt(&mut self, attempt: Attempt) {
        let _ = self.extensions_mut().insert(attempt);
    }
}

fn get_retry_after_duration(headers: &HeaderMap, clock: &Clock) -> Option<Duration> {
    let retry_after_raw = headers.get_str_value(RETRY_AFTER)?;

    // First, try to parse as an integer (seconds)
    if let Ok(seconds) = retry_after_raw.parse::<u64>() {
        return Some(Duration::from_secs(seconds));
    }

    if let Ok(timestamp) = retry_after_raw.parse::<Rfc2822>() {
        let timestamp: SystemTime = timestamp.into();

        return Some(timestamp.duration_since(clock.system_time()).unwrap_or(Duration::ZERO));
    }

    None
}

mod sealed {
    use http::Response;

    #[expect(unnameable_types, reason = "intentional, sealed trait pattern")]
    pub trait Sealed {}
    impl<B> Sealed for Response<B> {}
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn retry_after_seconds_value_ok() {
        let mut headers = HeaderMap::new();
        headers.insert(RETRY_AFTER, "120".parse().unwrap());

        // Clock is irrelevant for integer seconds.
        let clock = tick::Clock::new_frozen();
        let delay = get_retry_after_duration(&headers, &clock).unwrap();
        assert_eq!(delay, Duration::from_mins(2));
    }

    #[test]
    fn retry_after_date_future_ok() {
        // Use a frozen clock so "now" is stable
        let clock = tick::Clock::new_frozen();
        let now = clock.system_time();
        let future = now.checked_add(Duration::from_secs(5)).unwrap();

        let mut headers = HeaderMap::new();
        let rfc = Rfc2822::try_from(future).unwrap();
        headers.insert(RETRY_AFTER, rfc.to_string().parse().unwrap());

        let delay = get_retry_after_duration(&headers, &clock).unwrap();
        assert_eq!(delay, Duration::from_secs(5));
    }

    #[test]
    fn retry_after_date_in_past_returns_zero() {
        // Set a stable clock and create a timestamp 5s in the past
        let clock = Clock::new_frozen_at(SystemTime::UNIX_EPOCH + Duration::from_secs(10));
        let now = clock.system_time();
        let past = now.checked_sub(Duration::from_secs(5)).unwrap();

        let mut headers = HeaderMap::new();
        let rfc = Rfc2822::try_from(past).unwrap();
        headers.insert(RETRY_AFTER, rfc.to_string().parse().unwrap());

        let delay = get_retry_after_duration(&headers, &clock).unwrap();
        assert_eq!(delay, Duration::ZERO);
    }

    #[test]
    fn retry_after_missing_none() {
        let headers = HeaderMap::new();
        let clock = Clock::new_frozen();

        assert_eq!(get_retry_after_duration(&headers, &clock), None);
    }

    #[test]
    fn retry_after_invalid_none() {
        let mut headers = HeaderMap::new();
        headers.insert(RETRY_AFTER, "not-a-date".parse().unwrap());

        let clock = Clock::new_frozen();
        assert_eq!(get_retry_after_duration(&headers, &clock), None);
    }

    #[test]
    fn recovery_with_clock() {
        // Transient status without Retry-After
        let response = Response::builder().status(500).body(()).unwrap();
        assert_eq!(response.recovery_with_clock(&Clock::new_frozen()).kind(), RecoveryKind::Retry);

        // Transient status with Retry-After seconds
        let response = Response::builder().status(503).header(RETRY_AFTER, "60").body(()).unwrap();
        let recovery = response.recovery_with_clock(&Clock::new_frozen());
        assert_eq!(recovery.kind(), RecoveryKind::Retry);
        assert_eq!(recovery.get_delay(), Some(Duration::from_mins(1)));

        // Non-transient status
        let response = Response::builder().status(400).body(()).unwrap();
        assert_eq!(response.recovery_with_clock(&Clock::new_frozen()).kind(), RecoveryKind::Never);
    }

    #[test]
    fn attempt_returns_none_without_extension() {
        let response = Response::builder().status(200).body(()).unwrap();
        assert!(response.attempt().is_none());
    }

    #[test]
    fn set_attempt_then_attempt_round_trips() {
        let mut response = Response::builder().status(200).body(()).unwrap();

        response.set_attempt(Attempt::new(2, true));

        let attempt = response.attempt().expect("attempt should be recorded");
        assert_eq!(attempt.index(), 2);
        assert!(attempt.is_last());
    }

    #[test]
    fn set_attempt_overwrites_previous_attempt() {
        let mut response = Response::builder().status(200).body(()).unwrap();

        response.set_attempt(Attempt::new(0, false));
        response.set_attempt(Attempt::new(3, true));

        let attempt = response.attempt().expect("attempt should be recorded");
        assert_eq!(attempt.index(), 3);
        assert!(attempt.is_last());
    }
}
