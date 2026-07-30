// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use thread_aware::ThreadAware;

/// WinHTTP-specific TLS configuration.
///
/// Certificate and host-name validation are strict by default.
#[derive(Clone, Debug, Default, ThreadAware)]
#[non_exhaustive]
pub struct WinHttpTlsConfig {
    accept_invalid_certs: bool,
    accept_invalid_hostnames: bool,
}

impl WinHttpTlsConfig {
    /// Starts building WinHTTP-specific TLS configuration.
    #[must_use]
    pub fn builder() -> WinHttpTlsConfigBuilder {
        WinHttpTlsConfigBuilder { config: Self::default() }
    }

    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "configuration access is part of the transport module boundary")
    )]
    pub(crate) fn accepts_invalid_certs(&self) -> bool {
        self.accept_invalid_certs
    }

    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "configuration access is part of the transport module boundary")
    )]
    pub(crate) fn accepts_invalid_hostnames(&self) -> bool {
        self.accept_invalid_hostnames
    }
}

/// Builds [`WinHttpTlsConfig`].
#[derive(Clone, Debug, ThreadAware)]
#[non_exhaustive]
pub struct WinHttpTlsConfigBuilder {
    config: WinHttpTlsConfig,
}

impl WinHttpTlsConfigBuilder {
    /// Controls whether invalid server certificates are accepted.
    ///
    /// Enabling this option disables certificate trust, validity-period, and
    /// intended-usage checks. This is dangerous and should be limited to
    /// controlled scenarios.
    #[must_use]
    pub fn accept_invalid_certs(mut self, accept: bool) -> Self {
        self.config.accept_invalid_certs = accept;
        self
    }

    /// Controls whether invalid server host names are accepted.
    ///
    /// Enabling this option disables certificate host-name validation. This is
    /// dangerous and should be limited to controlled scenarios.
    #[must_use]
    pub fn accept_invalid_hostnames(mut self, accept: bool) -> Self {
        self.config.accept_invalid_hostnames = accept;
        self
    }

    /// Builds the WinHTTP-specific TLS configuration.
    #[must_use]
    pub fn build(self) -> WinHttpTlsConfig {
        self.config
    }
}

#[cfg(test)]
mod tests {
    use std::fmt::Debug;

    use static_assertions::assert_impl_all;
    use thread_aware::ThreadAware;

    use super::{WinHttpTlsConfig, WinHttpTlsConfigBuilder};

    assert_impl_all!(WinHttpTlsConfig: Send, Sync, Clone, Debug, Default, ThreadAware);
    assert_impl_all!(WinHttpTlsConfigBuilder: Send, Sync, Clone, Debug, ThreadAware);

    #[test]
    fn default_uses_strict_validation() {
        let config = WinHttpTlsConfig::default();

        assert!(!config.accepts_invalid_certs());
        assert!(!config.accepts_invalid_hostnames());
    }

    #[test]
    fn builder_sets_validation_relaxations_independently() {
        let invalid_certs = WinHttpTlsConfig::builder().accept_invalid_certs(true).build();
        assert!(invalid_certs.accepts_invalid_certs());
        assert!(!invalid_certs.accepts_invalid_hostnames());

        let invalid_hostnames = WinHttpTlsConfig::builder().accept_invalid_hostnames(true).build();
        assert!(!invalid_hostnames.accepts_invalid_certs());
        assert!(invalid_hostnames.accepts_invalid_hostnames());
    }
}
