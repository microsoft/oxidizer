// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Identity diagnostics and backend option propagation.

use fetch_tls::ClientIdentity;

#[test]
fn gamma_survivor_identity_debug_redacts_material_and_names_fields() {
    let identity = ClientIdentity::from_der([b"leaf-secret".as_slice(), b"issuer-secret"], b"key-secret");
    assert_eq!(
        format!("{identity:?}"),
        "ClientIdentity { cert_chain_len: 2, private_key: \"<redacted>\" }"
    );
    let expected = format!("{identity:?}");
    let cloned = identity.clone();
    drop(identity);
    assert_eq!(format!("{cloned:?}"), expected);
}

#[test]
fn gamma_survivor_identity_malformed_certificate_is_error_not_panic() {
    let error = ClientIdentity::from_pem(
        b"-----BEGIN CERTIFICATE-----\n!!!\n-----END CERTIFICATE-----\n",
        b"-----BEGIN PRIVATE KEY-----\nMAA=\n-----END PRIVATE KEY-----\n",
    )
    .unwrap_err();
    assert!(!error.to_string().is_empty());
}

#[cfg(feature = "native-tls")]
#[test]
#[cfg_attr(miri, ignore = "native tests cover platform TLS connector construction")]
fn gamma_survivor_ipv6_host_override_matches_other_ports() {
    use fetch_tls::{TlsBackend, TlsBackendBuilder, TlsOptions};

    let options = TlsOptions::builder_native_tls()
        .server_name_override("[::1]", "host.example.test")
        .server_name_override("[::1]:8443", "port.example.test")
        .build();
    let backend = match TlsBackendBuilder::new().build_backend(options).unwrap() {
        TlsBackend::NativeTls(backend) => backend,
        #[cfg(feature = "rustls")]
        TlsBackend::Rustls(_) => panic!("native options must select native TLS"),
    };
    assert_eq!(backend.server_name_for(&"[::1]:9443".parse().unwrap()), Some("host.example.test"));
    assert_eq!(backend.server_name_for(&"[::1]:8443".parse().unwrap()), Some("port.example.test"));
    assert_eq!(backend.server_name_for(&"[::2]:9443".parse().unwrap()), None);
}

#[cfg(feature = "native-tls")]
#[test]
#[cfg_attr(miri, ignore = "native tests cover identity validation through platform TLS FFI")]
fn gamma_survivor_auto_backend_preserves_invalid_shared_identity_error() {
    use fetch_tls::{TlsBackendBuilder, TlsOptions};

    let options = TlsOptions::builder()
        .client_identity(ClientIdentity::from_der([b"invalid certificate"], b"invalid key"))
        .build();
    TlsBackendBuilder::new()
        .defaults_to_native_tls()
        .build_backend(options)
        .unwrap_err();
}

#[cfg(feature = "rustls")]
mod rustls_contracts {
    use std::sync::Arc;

    use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
    use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
    use rustls::{DigitallySignedStruct, SignatureScheme};

    #[derive(Debug)]
    struct NoHandshake;

    impl ServerCertVerifier for NoHandshake {
        fn verify_server_cert(
            &self,
            _: &CertificateDer<'_>,
            _: &[CertificateDer<'_>],
            _: &ServerName<'_>,
            _: &[u8],
            _: UnixTime,
        ) -> Result<ServerCertVerified, rustls::Error> {
            Err(rustls::Error::General("unexpected handshake".into()))
        }

        fn verify_tls12_signature(
            &self,
            _: &[u8],
            _: &CertificateDer<'_>,
            _: &DigitallySignedStruct,
        ) -> Result<HandshakeSignatureValid, rustls::Error> {
            Err(rustls::Error::General("unexpected handshake".into()))
        }

        fn verify_tls13_signature(
            &self,
            _: &[u8],
            _: &CertificateDer<'_>,
            _: &DigitallySignedStruct,
        ) -> Result<HandshakeSignatureValid, rustls::Error> {
            Err(rustls::Error::General("unexpected handshake".into()))
        }

        fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
            vec![SignatureScheme::ED25519]
        }
    }

    #[test]
    fn gamma_survivor_rustls_empty_cipher_suite_provider_returns_error() {
        let mut crypto = rustls::crypto::aws_lc_rs::default_provider();
        crypto.cipher_suites.clear();
        let defaults = fetch_tls::TlsBackendBuilder::new().configure_rustls(Arc::new(crypto), Arc::new(NoHandshake));
        let error = defaults.build_backend(fetch_tls::TlsOptions::new_rustls()).unwrap_err();
        assert!(!error.to_string().is_empty());
    }
}
