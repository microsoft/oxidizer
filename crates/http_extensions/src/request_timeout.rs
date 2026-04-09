// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::time::Duration;

/// A request-level timeout that can be attached to HTTP requests as an extension.
///
/// This timeout represents the maximum time allowed for the entire request/response cycle,
/// including receiving the response headers and reading all data from the HTTP body. This
/// is different from the body timeout that only limits the time spent streaming the
/// response body. Use this to set a per-request deadline that covers connection, sending,
/// and receiving.
///
/// # Example
///
/// ```
/// use std::time::Duration;
///
/// use http_extensions::HttpRequestBuilder;
///
/// let request = HttpRequestBuilder::new_fake()
///     .get("https://example.com")
///     .timeout(Duration::from_secs(30))
///     .build()
///     .unwrap();
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RequestTimeout(pub Duration);

impl RequestTimeout {
    /// Creates a new `RequestTimeout` with the given duration.
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

impl From<Duration> for RequestTimeout {
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
        let timeout = RequestTimeout::new(Duration::from_secs(30));
        assert_eq!(timeout.0, Duration::from_secs(30));
    }

    #[test]
    fn duration_returns_inner_value() {
        let timeout = RequestTimeout::new(Duration::from_millis(500));
        assert_eq!(timeout.duration(), Duration::from_millis(500));
    }

    #[test]
    fn from_duration() {
        let timeout: RequestTimeout = Duration::from_secs(10).into();
        assert_eq!(timeout.duration(), Duration::from_secs(10));
    }

    #[test]
    fn clone_and_copy() {
        let timeout = RequestTimeout::new(Duration::from_secs(5));
        let cloned = timeout.clone();
        let copied = timeout;

        assert_eq!(timeout, cloned);
        assert_eq!(timeout, copied);
    }

    #[test]
    fn debug_formatting() {
        let timeout = RequestTimeout::new(Duration::from_secs(42));
        let debug = format!("{timeout:?}");
        assert!(debug.contains("RequestTimeout"));
        assert!(debug.contains("42"));
    }
}
