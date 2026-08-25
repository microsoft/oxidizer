// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Maps caller-facing TLS relaxations onto native `WinHTTP` security flags.
//!
//! `fetch`'s generic TLS options describe rustls or native-tls objects that
//! cannot configure a Schannel-backed `WinHTTP` request, so this transport
//! exposes its own configuration type instead (design.md section 4,
//! design.md section 1.2). [`WinHttpTlsConfig`] is the caller-facing half and
//! [`security_flags`] is the native half: it produces the
//! `WINHTTP_OPTION_SECURITY_FLAGS` mask that [`crate::request`] applies to a
//! request handle before sending (implementation.md section 10.2).
//!
//! Both halves live together because the flag mask is the entire meaning of
//! the configuration - a relaxation that maps to no flag would silently do
//! nothing, and keeping the mapping next to the documentation that promises
//! each option makes the blast radius of that option easy to review.

use thread_aware::ThreadAware;
use windows::Win32::Networking::WinHttp::{
    SECURITY_FLAG_IGNORE_CERT_CN_INVALID, SECURITY_FLAG_IGNORE_CERT_DATE_INVALID, SECURITY_FLAG_IGNORE_CERT_WRONG_USAGE,
    SECURITY_FLAG_IGNORE_UNKNOWN_CA,
};

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

/// Certificate checks WinHTTP skips when the caller accepts invalid certificates.
///
/// Every operand is a distinct bit, so `|` and `^` compute the same value and a
/// mutation between them is equivalent rather than a defect.
#[cfg_attr(test, mutants::skip)] // Disjoint-bit union: `|` and `^` are interchangeable, so operator mutants are equivalent.
const fn ignored_certificate_checks() -> u32 {
    SECURITY_FLAG_IGNORE_UNKNOWN_CA | SECURITY_FLAG_IGNORE_CERT_WRONG_USAGE | SECURITY_FLAG_IGNORE_CERT_DATE_INVALID
}

pub(crate) fn security_flags(config: &WinHttpTlsConfig) -> u32 {
    let mut flags = 0;

    if config.accepts_invalid_certs() {
        flags |= ignored_certificate_checks();
    }
    if config.accepts_invalid_hostnames() {
        flags |= SECURITY_FLAG_IGNORE_CERT_CN_INVALID;
    }

    flags
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::fmt::Debug;
    use std::panic::{RefUnwindSafe, UnwindSafe};

    use static_assertions::assert_impl_all;
    use thread_aware::ThreadAware;

    use super::{
        SECURITY_FLAG_IGNORE_CERT_CN_INVALID, SECURITY_FLAG_IGNORE_CERT_DATE_INVALID, SECURITY_FLAG_IGNORE_CERT_WRONG_USAGE,
        SECURITY_FLAG_IGNORE_UNKNOWN_CA, WinHttpTlsConfig, WinHttpTlsConfigBuilder, security_flags,
    };

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

    #[test]
    fn security_flags_combine_windows_sdk_flags() {
        assert_eq!(security_flags(&WinHttpTlsConfig::default()), 0);
        assert_eq!(
            security_flags(&WinHttpTlsConfig::builder().accept_invalid_certs(true).build()),
            SECURITY_FLAG_IGNORE_UNKNOWN_CA | SECURITY_FLAG_IGNORE_CERT_WRONG_USAGE | SECURITY_FLAG_IGNORE_CERT_DATE_INVALID
        );
        assert_eq!(
            security_flags(&WinHttpTlsConfig::builder().accept_invalid_hostnames(true).build()),
            SECURITY_FLAG_IGNORE_CERT_CN_INVALID
        );
        assert_eq!(
            security_flags(
                &WinHttpTlsConfig::builder()
                    .accept_invalid_certs(true)
                    .accept_invalid_hostnames(true)
                    .build()
            ),
            SECURITY_FLAG_IGNORE_UNKNOWN_CA
                | SECURITY_FLAG_IGNORE_CERT_WRONG_USAGE
                | SECURITY_FLAG_IGNORE_CERT_CN_INVALID
                | SECURITY_FLAG_IGNORE_CERT_DATE_INVALID
        );
    }
}
