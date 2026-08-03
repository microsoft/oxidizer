// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use thread_aware::ThreadAware;

/// Configures TLS behavior that only the WinHTTP transport can apply.
///
/// Generic `fetch` TLS options describe rustls or native-tls objects that cannot
/// configure requests backed by Schannel in WinHTTP, so this transport-specific
/// value maps directly to WinHTTP request security flags instead.
///
/// Certificate-chain relaxations and host-name relaxation are independent and
/// both default to strict validation. The former covers an unknown authority,
/// invalid validity period, or wrong intended usage; the latter covers a host
/// name mismatch. Other TLS failures remain enforced.
///
/// Client certificates, server-certificate inspection, and certificate pinning
/// are not supported.
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

    pub(crate) fn accepts_invalid_certs(&self) -> bool {
        self.accept_invalid_certs
    }

    pub(crate) fn accepts_invalid_hostnames(&self) -> bool {
        self.accept_invalid_hostnames
    }
}

/// Builds [`WinHttpTlsConfig`] from independent validation controls.
///
/// Settings remain strict unless the corresponding relaxation is explicitly
/// enabled.
#[derive(Clone, Debug, ThreadAware)]
#[non_exhaustive]
pub struct WinHttpTlsConfigBuilder {
    config: WinHttpTlsConfig,
}

impl WinHttpTlsConfigBuilder {
    /// Controls whether invalid server certificates are accepted.
    ///
    /// Relaxes selected server-certificate validation failures.
    ///
    /// This covers an unknown CA, an invalid validity period, and an invalid
    /// intended usage. Other Schannel failures remain enforced.
    ///
    /// This is dangerous and should be limited to controlled scenarios.
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
    use std::panic::{RefUnwindSafe, UnwindSafe};

    use static_assertions::assert_impl_all;
    use thread_aware::ThreadAware;

    use super::{WinHttpTlsConfig, WinHttpTlsConfigBuilder};

    assert_impl_all!(WinHttpTlsConfig: Send, Sync, Clone, Debug, Default, ThreadAware, UnwindSafe, RefUnwindSafe);
    assert_impl_all!(WinHttpTlsConfigBuilder: Send, Sync, Clone, Debug, ThreadAware, UnwindSafe, RefUnwindSafe);

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
