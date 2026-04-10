// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::time::Duration;

/// A body-level idle timeout that can be attached to HTTP requests as an extension.
///
/// This timeout limits how long the client will wait between chunks of response body
/// data. The timer resets every time the body makes progress, so only idle periods
/// (no data received) count toward the timeout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BodyTimeout(Duration);

impl BodyTimeout {
    /// Creates a new `BodyTimeout` with the given duration.
    #[must_use]
    pub fn new(duration: Duration) -> Self {
        Self(duration)
    }

    /// Returns the timeout duration.
    #[must_use]
    pub fn duration(&self) -> Duration {
        self.0
    }
}

impl From<Duration> for BodyTimeout {
    fn from(duration: Duration) -> Self {
        Self::new(duration)
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn new_creates_timeout_with_given_duration() {
        let timeout = BodyTimeout::new(Duration::from_secs(30));
        assert_eq!(timeout.duration(), Duration::from_secs(30));
    }

    #[test]
    fn duration_returns_inner_value() {
        let timeout = BodyTimeout::new(Duration::from_millis(500));
        assert_eq!(timeout.duration(), Duration::from_millis(500));
    }

    #[test]
    fn from_duration() {
        let timeout: BodyTimeout = Duration::from_secs(10).into();
        assert_eq!(timeout.duration(), Duration::from_secs(10));
    }

    #[test]
    fn clone_and_copy() {
        let timeout = BodyTimeout::new(Duration::from_secs(5));
        let cloned = timeout;
        let copied = timeout;

        assert_eq!(timeout, cloned);
        assert_eq!(timeout, copied);
    }

    #[test]
    fn debug_formatting() {
        let timeout = BodyTimeout::new(Duration::from_secs(42));
        let debug = format!("{timeout:?}");
        assert!(debug.contains("BodyTimeout"));
        assert!(debug.contains("42"));
    }
}
