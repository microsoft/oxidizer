// Copyright (c) Microsoft Corporation.

use std::time::Duration;

/// Arguments passed to timeout callback functions.
///
/// Contains information about the timeout event that can be used for logging,
/// metrics, or other side effects when a timeout occurs.
#[derive(Debug)]
#[non_exhaustive]
pub struct OnTimeoutArgs {
    pub(super) timeout: Duration,
}

impl OnTimeoutArgs {
    /// Returns the timeout duration that was exceeded.
    ///
    /// This is the duration after which the operation was canceled.
    #[must_use]
    pub fn timeout(&self) -> Duration {
        self.timeout
    }
}

/// Arguments passed to timeout override functions.
#[derive(Debug)]
#[non_exhaustive]
pub struct TimeoutOverrideArgs {
    pub(super) default_timeout: Duration,
}

impl TimeoutOverrideArgs {
    /// Returns the default timeout duration configured for the middleware.
    ///
    /// This can be used as a base value when calculating dynamic timeouts, or to
    /// explicitly reuse the default by returning `None` from the override closure.
    #[must_use]
    pub fn default_timeout(&self) -> Duration {
        self.default_timeout
    }
}

/// Arguments passed to timeout output functions.
///
/// Contains information about the timeout event that occurred,
/// which can be used to create appropriate timeout responses.
#[derive(Debug)]
pub struct TimeoutOutputArgs {
    pub(super) timeout: Duration,
}

impl TimeoutOutputArgs {
    /// Returns the timeout duration that was exceeded.
    ///
    /// This is the duration after which the operation was canceled.
    /// It can be used to provide detailed timeout information in the response.
    #[must_use]
    pub fn timeout(&self) -> Duration {
        self.timeout
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_timeout_ok() {
        let args = TimeoutOverrideArgs {
            default_timeout: Duration::from_secs(5),
        };

        assert_eq!(args.default_timeout(), Duration::from_secs(5));
    }
}
