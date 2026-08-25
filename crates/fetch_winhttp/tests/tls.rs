// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Transport security behaviour over localhost.
//!
//! Covers how the configured certificate and hostname validation relaxations reach the live TLS
//! handshake.

#![cfg(windows)]

use fetch_winhttp::WinHttpTlsConfig;
use fetch_winhttp_testing::{ResponsePlan, TestServer, client};
use http::Version;

#[cfg_attr(miri, ignore)]
#[test]
fn tls_validation_relaxations_are_independent() {
    let valid_name = TestServer::https([ResponsePlan::ok("valid name accepted")], &["localhost"]);
    let strict = client(&[Version::HTTP_11], WinHttpTlsConfig::default());
    futures::executor::block_on(strict.client.get(valid_name.url("/strict")).fetch()).unwrap_err();
    let invalid_certs = client(&[Version::HTTP_11], WinHttpTlsConfig::builder().accept_invalid_certs(true).build());
    let response = futures::executor::block_on(invalid_certs.client.get(valid_name.url("/accepted")).fetch_text_body()).unwrap();
    assert_eq!(response, "valid name accepted");

    let wrong_name = TestServer::https([ResponsePlan::ok("both accepted")], &["different.invalid"]);
    let invalid_certs = client(&[Version::HTTP_11], WinHttpTlsConfig::builder().accept_invalid_certs(true).build());
    futures::executor::block_on(invalid_certs.client.get(wrong_name.url("/certificate-only")).fetch()).unwrap_err();
    let invalid_hostnames = client(
        &[Version::HTTP_11],
        WinHttpTlsConfig::builder().accept_invalid_hostnames(true).build(),
    );
    futures::executor::block_on(invalid_hostnames.client.get(wrong_name.url("/hostname-only")).fetch()).unwrap_err();
    let both = client(
        &[Version::HTTP_11],
        WinHttpTlsConfig::builder()
            .accept_invalid_certs(true)
            .accept_invalid_hostnames(true)
            .build(),
    );
    let response = futures::executor::block_on(both.client.get(wrong_name.url("/both")).fetch_text_body()).unwrap();
    assert_eq!(response, "both accepted");
}
