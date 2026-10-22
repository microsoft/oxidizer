// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::time::Duration;

/// Provides data to timeout callbacks.
///
/// Use this data for logs, metrics, or other actions after a timeout.
#[derive(Debug)]
#[non_exhaustive]
pub struct OnTimeoutArgs {
    pub(super) timeout: Duration,
}

impl OnTimeoutArgs {
    /// Returns the timeout duration that was exceeded.
    ///
    /// The middleware canceled the operation after this duration.
    #[must_use]
    pub fn timeout(&self) -> Duration {
        self.timeout
    }
}

/// Provides data to timeout override functions.
#[derive(Debug)]
#[non_exhaustive]
pub struct TimeoutOverrideArgs {
    pub(super) default_timeout: Duration,
}

impl TimeoutOverrideArgs {
    /// Returns the default timeout duration configured for the middleware.
    ///
    /// Use this value to calculate a dynamic timeout.
    /// Return `None` from the override function to reuse it.
    #[must_use]
    pub fn default_timeout(&self) -> Duration {
        self.default_timeout
    }
}

/// Provides data to timeout output functions.
///
/// Use this data to create output after a timeout.
#[derive(Debug)]
pub struct TimeoutOutputArgs {
    pub(super) timeout: Duration,
}

impl TimeoutOutputArgs {
    /// Returns the timeout duration that was exceeded.
    ///
    /// The middleware canceled the operation after this duration.
    /// You can include it in the timeout output.
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
