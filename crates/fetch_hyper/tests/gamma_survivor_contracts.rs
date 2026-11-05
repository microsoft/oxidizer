// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! TLS identity configuration without opening a connection.

testing_aids::init_tracing!();

#[cfg(feature = "native-tls")]
#[test]
fn gamma_survivor_native_identity_accepts_generated_der_certificate_and_pkcs8_key() {
    let key = rcgen::KeyPair::generate_for(&rcgen::PKCS_RSA_SHA256).unwrap();
    let certificate = rcgen::CertificateParams::new(["example.invalid".to_owned()])
        .unwrap()
        .self_signed(&key)
        .unwrap();
    let identity = fetch_tls::ClientIdentity::from_der([certificate.der().to_vec()], key.serialize_der());
    let options = fetch_tls::TlsOptions::builder_native_tls().client_identity(identity).build();
    let backend = fetch_tls::TlsBackendBuilder::new().build_backend(options).unwrap();
    assert!(matches!(backend, fetch_tls::TlsBackend::NativeTls(_)));
}
