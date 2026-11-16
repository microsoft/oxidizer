// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Certificate details from failed rustls verification. The supplied verifier still makes every validation decision.

use std::fmt::Write as _;
use std::sync::Arc;

use ohno::EnrichableExt;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{DigitallySignedStruct, DistinguishedName, SignatureScheme};
use sha2::{Digest, Sha256};

#[derive(Debug)]
pub(crate) struct DiagnosticVerifier(pub(crate) Arc<dyn ServerCertVerifier>);

#[ohno::error]
/// A failed certificate check, with peer certificate details and the original error as its source.
pub struct CertificateVerificationError;

fn enrich(error: rustls::Error, certificate: &CertificateDer<'_>) -> rustls::Error {
    let mut fingerprint = String::with_capacity(64);
    for byte in Sha256::digest(certificate.as_ref()) {
        write!(&mut fingerprint, "{byte:02x}").expect("writing to a String cannot fail");
    }
    let (subject, sans) = match x509_parser::parse_x509_certificate(certificate.as_ref()) {
        Ok((_, parsed)) => {
            let sans = match parsed.subject_alternative_name() {
                Ok(Some(extension)) => format!("{:?}", extension.value.general_names),
                Ok(None) => "[]".to_owned(),
                Err(_) => "unavailable (invalid SAN extension)".to_owned(),
            };
            (format!("{:?}", parsed.subject().to_string()), sans)
        }
        Err(_) => (
            "unavailable (invalid certificate DER)".to_owned(),
            "unavailable (invalid certificate DER)".to_owned(),
        ),
    };
    let error = CertificateVerificationError::caused_by(error).enrich(format!(
        "TLS peer certificate (unverified): peer_certificate_subject={subject}, \
         peer_certificate_sans={sans}, peer_certificate_sha256={fingerprint}"
    ));
    rustls::Error::Other(rustls::OtherError(Arc::new(error)))
}

impl ServerCertVerifier for DiagnosticVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        ocsp_response: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        self.0
            .verify_server_cert(end_entity, intermediates, server_name, ocsp_response, now)
            .map_err(|error| enrich(error, end_entity))
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.0
            .verify_tls12_signature(message, cert, dss)
            .map_err(|error| enrich(error, cert))
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.0
            .verify_tls13_signature(message, cert, dss)
            .map_err(|error| enrich(error, cert))
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.0.supported_verify_schemes()
    }

    fn requires_raw_public_keys(&self) -> bool {
        self.0.requires_raw_public_keys()
    }

    fn root_hint_subjects(&self) -> Option<&[DistinguishedName]> {
        self.0.root_hint_subjects()
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error as _;

    use super::*;

    #[test]
    fn malformed_certificate_keeps_its_fingerprint_and_original_error() {
        let error = enrich(
            rustls::Error::InvalidCertificate(rustls::CertificateError::BadEncoding),
            &CertificateDer::from(&b"abc"[..]),
        );
        let message = error.to_string();
        assert!(message.contains("peer_certificate_subject=unavailable"), "{message}");
        assert!(message.contains("peer_certificate_sans=unavailable"), "{message}");
        assert!(
            message.contains("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"),
            "{message}"
        );
        let rustls::Error::Other(other) = error else {
            panic!("expected diagnostic carrier");
        };
        assert!(matches!(
            other.0.source().unwrap().downcast_ref::<rustls::Error>(),
            Some(rustls::Error::InvalidCertificate(rustls::CertificateError::BadEncoding))
        ));
    }

    #[test]
    fn successful_custom_verification_is_unchanged() {
        let inner = crate::testing::AcceptAllServerCertVerifier;
        let verifier = DiagnosticVerifier(Arc::new(inner));
        verifier
            .verify_server_cert(
                &CertificateDer::from(&b"custom-verifier-input"[..]),
                &[],
                &ServerName::try_from("expected.example.test").unwrap(),
                &[],
                UnixTime::since_unix_epoch(std::time::Duration::ZERO),
            )
            .unwrap();
    }
}
