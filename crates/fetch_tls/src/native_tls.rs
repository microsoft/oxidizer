// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Platform native TLS backend configuration and builder integration.

use std::collections::HashMap;

use http::uri::Authority;
use native_tls::TlsConnector;

use crate::TlsBackendBuilder;
use crate::alpn::map_to_alpn;
use crate::backend::{BackendError, server_name_override_key};
use crate::options::{SharedOptions, TlsOptions, TlsOptionsBuilder, TlsOptionsKind};

/// Platform native TLS backend.
#[derive(Clone, Default)]
#[non_exhaustive]
pub struct NativeTlsOptions {
    root_certificates: Vec<native_tls::Certificate>,
    server_name_overrides: HashMap<String, String>,
}

impl std::fmt::Debug for NativeTlsOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NativeTlsOptions")
            .field("root_certificate_count", &self.root_certificates.len())
            .field("server_name_override_count", &self.server_name_overrides.len())
            .finish()
    }
}

impl NativeTlsOptions {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Materializes this configuration into a [`native_tls::TlsConnector`].
    pub(crate) fn build(
        self,
        defaults: &TlsBackendBuilder,
        shared: &SharedOptions,
    ) -> Result<(TlsConnector, HashMap<String, String>), BackendError> {
        let mut builder = native_tls::TlsConnector::builder();
        builder
            .request_alpns(map_to_alpn(shared.resolved_supported_http_versions(defaults)))
            .min_protocol_version(Some(native_tls::Protocol::Tlsv12));

        for certificate in self.root_certificates {
            builder.add_root_certificate(certificate);
        }

        if let Some(identity) = shared.client_identity.as_ref() {
            identity
                .build_native_identity()
                .map(|i| {
                    builder.identity(i);
                })
                .map_err(BackendError::caused_by)?;
        }

        let connector = builder.build().map_err(BackendError::caused_by)?;
        Ok((connector, self.server_name_overrides))
    }
}

impl TlsOptions {
    /// Creates a builder for the platform native TLS backend.
    pub fn builder_native_tls() -> TlsOptionsBuilder<NativeTlsOptions> {
        TlsOptionsBuilder {
            backend: NativeTlsOptions::new(),
            shared: SharedOptions::default(),
        }
    }

    /// Creates [`TlsOptions`] for the platform native TLS backend using
    /// default settings.
    ///
    /// Equivalent to `TlsOptions::builder_native_tls().build()`. Use
    /// [`TlsOptions::builder_native_tls`] when you need to customize the
    /// configuration before building.
    pub fn new_native_tls() -> Self {
        Self::builder_native_tls().build()
    }
}

/// Wraps a pre-built [`native_tls::TlsConnector`] as [`TlsOptions`].
impl From<TlsConnector> for TlsOptions {
    fn from(connector: TlsConnector) -> Self {
        Self {
            inner: TlsOptionsKind::PreConfigured(connector.into()),
            shared: SharedOptions::default(),
        }
    }
}

impl TlsOptionsBuilder<NativeTlsOptions> {
    /// Adds a trusted root certificate for native TLS server validation.
    pub fn root_certificate(mut self, certificate: native_tls::Certificate) -> Self {
        self.backend.root_certificates.push(certificate);
        self
    }

    /// Overrides the server name used for native TLS certificate validation
    /// and Server Name Indication (`SNI`) when connecting to `endpoint`.
    ///
    /// `SNI` is the server name sent to the peer during the TLS handshake.
    ///
    /// The HTTP request, connection destination, `Host` header, and connection
    /// pool key continue to use the real endpoint. Only the name passed to
    /// [`native_tls::TlsConnector::connect`] is replaced.
    ///
    /// The endpoint is matched against the request URI authority. An endpoint
    /// with a port (for example, `localhost:8443`) applies only to that exact
    /// authority. A host-only endpoint (for example, `localhost`) applies to
    /// every port unless an exact authority override is also configured.
    ///
    /// ```
    /// # use fetch_tls::TlsOptions;
    /// let tls = TlsOptions::builder_native_tls()
    ///     .server_name_override("localhost:8443", "tvs.example.test")
    ///     .build();
    /// ```
    ///
    /// # Panics
    ///
    /// Panics if `endpoint` is not a valid URI authority, if an explicit port
    /// cannot be represented as a `u16`, or if `server_name` is empty.
    pub fn server_name_override<E>(mut self, endpoint: E, server_name: impl Into<String>) -> Self
    where
        E: TryInto<Authority>,
        E::Error: std::fmt::Debug,
    {
        let endpoint = endpoint
            .try_into()
            .expect("native TLS server name override endpoint must be a valid URI authority");
        assert!(
            !Self::has_explicit_port(&endpoint) || endpoint.port_u16().is_some(),
            "native TLS server name override endpoint port must fit in a u16"
        );
        let server_name = server_name.into();
        assert!(!server_name.is_empty(), "native TLS server name override cannot be empty");
        assert!(
            !server_name.contains('\0'),
            "native TLS server name override cannot contain a NUL character"
        );
        self.backend
            .server_name_overrides
            .insert(server_name_override_key(&endpoint), server_name);
        self
    }

    fn has_explicit_port(endpoint: &Authority) -> bool {
        let authority = endpoint.as_str();
        if authority.starts_with('[') {
            authority
                .find(']')
                .and_then(|closing_bracket| authority.as_bytes().get(closing_bracket + 1))
                == Some(&b':')
        } else {
            authority.contains(':')
        }
    }

    /// Builds the final [`TlsOptions`] with the native TLS backend.
    pub fn build(self) -> TlsOptions {
        TlsOptions {
            inner: TlsOptionsKind::NativeTls(self.backend),
            shared: self.shared,
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use crate::client_identity::ClientIdentity;

    #[test]
    fn builder_native_tls_starts_empty() {
        let builder = TlsOptions::builder_native_tls();
        assert!(builder.shared.client_identity.is_none());
        assert!(builder.backend.root_certificates.is_empty());
        assert!(builder.backend.server_name_overrides.is_empty());
    }

    #[test]
    fn build_produces_native_tls_options() {
        let tls = TlsOptions::builder_native_tls().build();
        assert!(matches!(tls.inner, TlsOptionsKind::NativeTls(_)));
    }

    #[test]
    fn new_native_tls_produces_native_tls_options() {
        let tls = TlsOptions::new_native_tls();
        assert!(matches!(tls.inner, TlsOptionsKind::NativeTls(_)));
        assert!(tls.shared.client_identity.is_none());
    }

    #[test]
    #[cfg_attr(miri, ignore)] // native-tls touches platform TLS FFI
    fn tls_options_from_tls_connector_wraps_as_preconfigured() {
        let connector = native_tls::TlsConnector::builder().build().expect("builds");
        let tls = TlsOptions::from(connector);
        assert!(matches!(tls.inner, TlsOptionsKind::PreConfigured(_)));
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn build_fails_for_invalid_client_identity() {
        let identity = ClientIdentity::from_der(vec![vec![0xffu8, 0xff]], vec![0x30u8, 0x00]);
        let tls = TlsOptions::builder_native_tls().client_identity(identity).build();
        crate::TlsBackendBuilder::new()
            .build_backend(tls)
            .expect_err("expected build_backend to fail for invalid certificate");
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn build_produces_tls_connector() {
        let (connector, overrides) = NativeTlsOptions::new()
            .build(&crate::TlsBackendBuilder::new(), &SharedOptions::default())
            .unwrap();
        let _ = connector;
        assert!(overrides.is_empty());
    }

    // Self-signed RSA-2048 certificate and matching `PKCS#8` private key,
    // generated with `openssl req -x509 -newkey rsa:2048 -nodes` and then
    // `openssl pkcs8 -topk8 -nocrypt`. Used solely to exercise the
    // `builder.identity(...)` success branch with material that platform
    // native TLS implementations accept.
    const TEST_CERT_PEM: &[u8] = b"\
-----BEGIN CERTIFICATE-----\n\
MIIDATCCAemgAwIBAgIUeToZ/lJRIo2oAgWVrhsrXAfKhgMwDQYJKoZIhvcNAQEL\n\
BQAwDzENMAsGA1UEAwwEdGVzdDAgFw0yNjA2MDIwODEwMDZaGA8yMTI2MDUwOTA4\n\
MTAwNlowDzENMAsGA1UEAwwEdGVzdDCCASIwDQYJKoZIhvcNAQEBBQADggEPADCC\n\
AQoCggEBAMf3OvrZdXjAqQWlMarIojpwTlotdzdk9ayKRPWvxWXEH3qTFVuPedvC\n\
IvwCdB7Ptxns+92tJLB/okuxaP27lPsZj5d+eCEaZ0DfXrHAeyjYoQKGmXvd1J5S\n\
aWGAK1m6+giIZexWF+OgqzebtgBu/QFlID66UiLYyvq+rZW7hmzXreWtrvRdnEli\n\
nzd3m6fHN1Js9C8HX9WtxwDNQLcuMh38G+JU2MfE32e8AJLcJA4PQ64xWrQwxa1i\n\
52CSGXhY8g/SPIRPCD1QbDyh4OUnyBGiSNjrKrGQKnPwdGyg9dDkhfdlwVmCbOee\n\
VJKJ5ocybOJrqcAwDzvGdjRRsUB5GmECAwEAAaNTMFEwHQYDVR0OBBYEFKkZAizf\n\
UPBhPll31FWK4cWrkJluMB8GA1UdIwQYMBaAFKkZAizfUPBhPll31FWK4cWrkJlu\n\
MA8GA1UdEwEB/wQFMAMBAf8wDQYJKoZIhvcNAQELBQADggEBAJWwagVkk8ww6dvM\n\
NMKtlAPT1OWCbgY3HSGJ2VsbZYOqkKlYrbCwz6mr7AKB7oFDrUU8yrMxsmGULbJU\n\
Xtp6AX0iSgT2LRD+0D88ONPVJrv9LeQNalw/nWvlpeoUH27S8k8c6BFiHBIRFWOi\n\
eTyjnL0FS9He0f+IMAtlZOm9FoFWhiJET5+lWFwxflwiyHQNa0pbuuouXx09qFG5\n\
zlGzq0m2LEKrsEy2Q0YUh1/IdcJbCRwHiZNJWDcR3ZAEDKFKdGDcqiSWvICHR+lM\n\
AERHWw5sUzEOz0VjyThF5ZjxNl3bCQW5unSoFP5Dzv53nwc28Y9fDMV9ZOf4Uq+3\n\
xy3nEG0=\n\
-----END CERTIFICATE-----\n";

    const TEST_KEY_PEM: &[u8] = b"\
-----BEGIN PRIVATE KEY-----\n\
MIIEvQIBADANBgkqhkiG9w0BAQEFAASCBKcwggSjAgEAAoIBAQDH9zr62XV4wKkF\n\
pTGqyKI6cE5aLXc3ZPWsikT1r8VlxB96kxVbj3nbwiL8AnQez7cZ7PvdrSSwf6JL\n\
sWj9u5T7GY+XfnghGmdA316xwHso2KEChpl73dSeUmlhgCtZuvoIiGXsVhfjoKs3\n\
m7YAbv0BZSA+ulIi2Mr6vq2Vu4Zs163lra70XZxJYp83d5unxzdSbPQvB1/VrccA\n\
zUC3LjId/BviVNjHxN9nvACS3CQOD0OuMVq0MMWtYudgkhl4WPIP0jyETwg9UGw8\n\
oeDlJ8gRokjY6yqxkCpz8HRsoPXQ5IX3ZcFZgmznnlSSieaHMmzia6nAMA87xnY0\n\
UbFAeRphAgMBAAECggEAHM8vvihKVmVbdKbCKxrQ1J6Ni0x1mpN/GaaqHMOAKxNA\n\
fcZnE1PueIzVwX0RAcdnV+LevqdNW+hnl4Qt3jCCXFLobykXYZ2ONrh3yiAzHkVn\n\
nReLUV86XLz+1b9Df6ACmewc0xnsQy1IvbA/XnyfEu5g4OizILYfOFT0aVglG9UN\n\
mcG6usl6EupeJ7uQwclMsjswy+5wYJzLPtJqiH03aDEX35kDUyIIMi18eZ0NVCZE\n\
oYd/eK8//bQbumRBOxkPHsdPeVaiMeTV+PLBvvMYOlMBzdtGUgj4eJz6IzNfCFuM\n\
kYUZKjfydPrA7gz8bc0US09wRP1/DCuIH2rfoUvHYQKBgQD2IKzAq4BLz/leTGQk\n\
wMfBZVBZupmC2w79DQWCgmai1HR2R8Pp8wxxIuA8vGb6lr/Dj/PEO4Nzf0NayWV6\n\
AyYo+Y5TGBHIbq2EDxQ6lp8ByeQamKO1iqK0102POS6CiooUcQe4SQR72Q6TgjjY\n\
HUsoBI+yMmItbR1XpoCZe3R9BQKBgQDP/IyMjkSXVjTli4mp1oL/1cw2fHmtEbBO\n\
wvx3y+njMMoSXKukKHZm43o/0oMSjNlgIWHWc/CYz38cRnydvXk4CZCUirYTtRFs\n\
OT8OwO2MYgJI6CIJenRcB1A8Hr8Cw32cY3R3YNMOzT2oo+vphNAOyv70PAm/Dr1k\n\
URxwNRCGrQKBgAEDlXKdwkONsctPqUH1gV0sm64i9KrzWBZ2zUUCYIXfNjOejBIU\n\
rEJzEFVvuUTjBhs6JpjyXdJF/fMLzV05UhjtHkb9XGVk/1YB8eVj5XfOayAo7NO8\n\
pHr2QB2M8MIc7AC1joCV3GzeMg8thCpvxHV/v0/OoVTqlCpeRz1aoto5AoGAE+rX\n\
es5U+zkiL6lBMaZ9PQq4V69r54r+G0zI6J/6cetGLqP5O+s0C35VQq9iJfCfEHmh\n\
6OuJatjUD10gqepvJVKlKdRuw0xfssF4rG0FUqBAH8M7HzU+12FL6bX4DMezy7oq\n\
eRQoog49jVzFRsOVORVvfOwS8tzyfhzWYFh0kLECgYEAkJlzIIVKBxVsD7sr5QTG\n\
GBldds0S5PhDN2Q0XE//HYhJcny4Vh/Ll4zDrMnWVBVcLgIA6o2IG8nO3507f0YH\n\
T2R0y4+MiNZVtrHHlN46dUu27rSsZ11CExcqrzcyczaG7TmEGc5R1+iSnc5fjnes\n\
BqPhJRu3mmiSimquZzZdlYs=\n\
-----END PRIVATE KEY-----\n";

    #[test]
    #[cfg_attr(miri, ignore)] // native-tls touches platform TLS FFI
    fn build_applies_valid_client_identity() {
        // Exercises the `builder.identity(i)` success branch: a valid
        // certificate/key pair must round-trip through `build_native_identity`
        // and be installed on the `TlsConnector` builder without error.
        let identity = ClientIdentity::from_pem(TEST_CERT_PEM, TEST_KEY_PEM).expect("valid PEM should parse");
        let tls = TlsOptions::builder_native_tls().client_identity(identity).build();
        crate::TlsBackendBuilder::new()
            .build_backend(tls)
            .expect("build_backend should succeed for a valid client identity");
    }

    #[test]
    fn debug_renders_presence_only() {
        let s = format!("{:?}", NativeTlsOptions::new());
        assert!(s.contains("NativeTlsOptions"));
        assert!(s.contains("root_certificate_count: 0"));
        assert!(s.contains("server_name_override_count: 0"));
    }

    #[test]
    fn server_name_override_stores_endpoint_mapping() {
        let builder = TlsOptions::builder_native_tls()
            .server_name_override("LOCALHOST", "expected.example.test")
            .server_name_override("LOCALHOST:8443", "endpoint.example.test");

        assert_eq!(
            builder.backend.server_name_overrides.get("localhost").map(String::as_str),
            Some("expected.example.test")
        );
        assert_eq!(
            builder.backend.server_name_overrides.get("localhost:8443").map(String::as_str),
            Some("endpoint.example.test")
        );
    }

    #[test]
    #[should_panic(expected = "native TLS server name override cannot be empty")]
    fn server_name_override_rejects_empty_server_name() {
        let _ = TlsOptions::builder_native_tls().server_name_override("localhost", "");
    }

    #[test]
    #[should_panic(expected = "native TLS server name override cannot contain a NUL character")]
    fn server_name_override_rejects_embedded_nul() {
        let _ = TlsOptions::builder_native_tls().server_name_override("localhost", "good.example\0evil.example");
    }

    #[test]
    #[should_panic(expected = "native TLS server name override endpoint must be a valid URI authority")]
    fn server_name_override_rejects_invalid_endpoint() {
        let _ = TlsOptions::builder_native_tls().server_name_override("not a valid authority", "expected.example.test");
    }

    #[test]
    #[should_panic(expected = "native TLS server name override endpoint port must fit in a u16")]
    fn server_name_override_rejects_out_of_range_port() {
        let _ = TlsOptions::builder_native_tls().server_name_override("localhost:65536", "expected.example.test");
    }

    #[test]
    fn server_name_override_accepts_ipv6_without_port() {
        let builder = TlsOptions::builder_native_tls().server_name_override("[::1]", "expected.example.test");
        assert_eq!(
            builder.backend.server_name_overrides.get("::1").map(String::as_str),
            Some("expected.example.test")
        );
    }

    #[test]
    fn server_name_override_accepts_ipv6_with_port() {
        let builder = TlsOptions::builder_native_tls().server_name_override("[::1]:8443", "expected.example.test");
        assert_eq!(
            builder.backend.server_name_overrides.get("[::1]:8443").map(String::as_str),
            Some("expected.example.test")
        );
    }

    #[test]
    #[should_panic(expected = "native TLS server name override endpoint port must fit in a u16")]
    fn server_name_override_rejects_out_of_range_ipv6_port() {
        let _ = TlsOptions::builder_native_tls().server_name_override("[::1]:65536", "expected.example.test");
    }
}
