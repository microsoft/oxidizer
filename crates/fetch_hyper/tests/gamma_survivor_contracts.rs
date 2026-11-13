// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! TLS identity configuration without opening a connection.

testing_aids::init_tracing!();

#[cfg(all(feature = "native-tls", any(not(target_vendor = "apple"), target_os = "macos")))]
#[test]
#[cfg_attr(miri, ignore = "native tests cover platform TLS FFI")]
fn gamma_survivor_native_identity_accepts_der_certificate_and_pkcs8_key() {
    let certificate = include_bytes!("fixtures/native_identity_certificate.der");
    let key = include_bytes!("fixtures/native_identity_key.der");
    let identity = fetch_tls::ClientIdentity::from_der([certificate.to_vec()], key);
    let options = fetch_tls::TlsOptions::builder_native_tls().client_identity(identity).build();
    let backend = fetch_tls::TlsBackendBuilder::new().build_backend(options).unwrap();
    assert!(matches!(backend, fetch_tls::TlsBackend::NativeTls(_)));
}
