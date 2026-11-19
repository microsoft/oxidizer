// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Helpers for the unit tests in `fetch_tls`.
//!
//! These helpers compile only under `cfg(test)`. Do not use them in production.
//! They accept every server certificate and never provide a client certificate.

use std::sync::Arc;

use rustls::client::ResolvesClientCert;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::sign::CertifiedKey;
use rustls::{DigitallySignedStruct, SignatureScheme};

/// [`ServerCertVerifier`] that accepts every server certificate.
///
/// Use this type only to test TLS configuration without valid certificates.
/// Do not use it in production.
#[derive(Debug, Default)]
pub(crate) struct AcceptAllServerCertVerifier;

impl ServerCertVerifier for AcceptAllServerCertVerifier {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        vec![
            SignatureScheme::RSA_PKCS1_SHA256,
            SignatureScheme::ECDSA_NISTP256_SHA256,
            SignatureScheme::ED25519,
        ]
    }
}

/// [`ResolvesClientCert`] that never produces a client certificate.
///
/// Use this type to pass `ResolvesClientCert` through [`crate::RustlsOptions`]
/// without a real key.
#[derive(Debug, Default)]
pub(crate) struct NoClientCertResolver;

impl ResolvesClientCert for NoClientCertResolver {
    fn resolve(&self, _root_hint_subjects: &[&[u8]], _sigschemes: &[SignatureScheme]) -> Option<Arc<CertifiedKey>> {
        None
    }

    fn has_certs(&self) -> bool {
        false
    }
}
