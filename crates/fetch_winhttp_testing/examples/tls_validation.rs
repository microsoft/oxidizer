// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Configures TLS certificate validation for the WinHTTP transport.
//!
//! `fetch`'s generic `TlsOptions` describe a certificate store and client certificates that
//! Windows manages itself, so this transport does not act on them. TLS behavior it can control
//! is configured through `WinHttpTlsConfig` on `WinHttpDeps` instead. The default validates
//! strictly, and each relaxation is a separate opt-in: accepting an untrusted certificate does
//! not also accept a hostname mismatch.
//!
//! The example serves three certificates from a localhost fixture to show each combination.
//!
//! Run with `cargo run -p fetch_winhttp_testing --example tls_validation`.

fn main() {
    #[cfg(windows)]
    example::run();

    #[cfg(not(windows))]
    println!("fetch_winhttp is a Windows-only transport, so this example does nothing here.");
}

#[cfg(windows)]
mod example {
    use fetch_winhttp::WinHttpTlsConfig;
    use fetch_winhttp_testing::{ResponsePlan, TestServer, client};
    use http::Version;

    pub(super) fn run() {
        // A self-signed certificate for the right host: only the trust check needs relaxing.
        report(
            "strict validation, self-signed certificate",
            &["localhost"],
            WinHttpTlsConfig::default(),
        );
        report(
            "untrusted certificates accepted",
            &["localhost"],
            WinHttpTlsConfig::builder().accept_invalid_certs(true).build(),
        );

        // A certificate issued for another name: trust and hostname are distinct faults, so
        // relaxing only one of them still fails.
        report(
            "untrusted accepted, hostname still checked",
            &["other.example"],
            WinHttpTlsConfig::builder().accept_invalid_certs(true).build(),
        );
        report(
            "untrusted and hostname mismatch both accepted",
            &["other.example"],
            WinHttpTlsConfig::builder()
                .accept_invalid_certs(true)
                .accept_invalid_hostnames(true)
                .build(),
        );
    }

    fn report(label: &str, certificate_names: &[&str], tls: WinHttpTlsConfig) {
        let server = TestServer::https([ResponsePlan::ok("secured")], certificate_names);
        let test_client = client(&[Version::HTTP_11], tls);

        let outcome = match futures::executor::block_on(test_client.client.get(server.url("/tls")).fetch_text_body()) {
            Ok(body) => format!("accepted: {body}"),
            Err(error) => format!("rejected: {error}"),
        };

        println!("{label}: {outcome}");
        drop(server.finish());
    }
}
