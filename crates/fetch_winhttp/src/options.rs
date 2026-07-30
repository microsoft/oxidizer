// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::time::Duration;

use thread_aware::ThreadAware;

/// WinHTTP-specific transport options.
///
/// The default configuration leaves the native DNS resolution timeout
/// unlimited.
#[derive(Clone, Debug, Default, ThreadAware)]
#[non_exhaustive]
pub struct WinHttpOptions {
    resolve_timeout: Option<Duration>,
}

impl WinHttpOptions {
    /// Starts building WinHTTP-specific transport options.
    #[must_use]
    pub fn builder() -> WinHttpOptionsBuilder {
        WinHttpOptionsBuilder { options: Self::default() }
    }

    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "configuration access is part of the transport module boundary")
    )]
    pub(crate) fn resolve_timeout(&self) -> Option<Duration> {
        self.resolve_timeout
    }
}

/// Builds [`WinHttpOptions`].
#[derive(Clone, Debug, ThreadAware)]
#[non_exhaustive]
pub struct WinHttpOptionsBuilder {
    options: WinHttpOptions,
}

impl WinHttpOptionsBuilder {
    /// Sets the native DNS resolution timeout.
    ///
    /// This timeout covers DNS resolution only. Other request deadlines are
    /// configured through the generic `fetch` client options.
    #[must_use]
    pub fn resolve_timeout(mut self, timeout: Duration) -> Self {
        self.options.resolve_timeout = Some(timeout);
        self
    }

    /// Builds the WinHTTP-specific transport options.
    #[must_use]
    pub fn build(self) -> WinHttpOptions {
        self.options
    }
}

#[cfg(test)]
mod tests {
    use std::fmt::Debug;
    use std::time::Duration;

    use static_assertions::assert_impl_all;
    use thread_aware::ThreadAware;

    use super::{WinHttpOptions, WinHttpOptionsBuilder};

    assert_impl_all!(WinHttpOptions: Send, Sync, Clone, Debug, Default, ThreadAware);
    assert_impl_all!(WinHttpOptionsBuilder: Send, Sync, Clone, Debug, ThreadAware);

    #[test]
    fn default_leaves_resolve_timeout_unlimited() {
        assert_eq!(WinHttpOptions::default().resolve_timeout(), None);
    }

    #[test]
    fn builder_sets_resolve_timeout() {
        let timeout = Duration::from_secs(10);
        let options = WinHttpOptions::builder().resolve_timeout(timeout).build();

        assert_eq!(options.resolve_timeout(), Some(timeout));
    }
}
