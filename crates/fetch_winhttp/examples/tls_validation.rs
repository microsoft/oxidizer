// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Configures TLS certificate validation for the WinHTTP transport.
//!
//! Windows manages the certificate store and client certificates, so this
//! transport ignores the generic `TlsOptions`. Use `WinHttpTlsConfig` on
//! `WinHttpDeps` for supported TLS configuration. The default uses strict
//! validation. Each relaxation is independent.
//!
//! The example serves three certificates from a localhost fixture to show each combination.
//!
//! Run with `cargo run -p fetch_winhttp --example tls_validation`.

#[cfg(windows)]
#[tokio::main]
async fn main() -> Result<(), ohno::AppError> {
    example::run().await
}

#[cfg(not(windows))]
fn main() {
    println!("fetch_winhttp is a Windows-only transport, so this example does nothing here.");
}

#[cfg(windows)]
mod example {
    use fetch_winhttp::WinHttpTlsConfig;
    use fetch_winhttp_impl::testing::{ResponsePlan, TestServer, client};
    use http::Version;
    use tick::Clock;

    pub(super) async fn run() -> Result<(), ohno::AppError> {
        // This self-signed certificate uses the correct host name. Only trust
        // validation must be relaxed.
        report(
            "strict validation, self-signed certificate",
            &["localhost"],
            WinHttpTlsConfig::default(),
        )
        .await?;
        report(
            "untrusted certificates accepted",
            &["localhost"],
            WinHttpTlsConfig::builder().accept_invalid_certs(true).build(),
        )
        .await?;

        // This certificate uses another host name. Trust and host-name
        // validation are separate, so relaxing only one still fails.
        report(
            "untrusted accepted, hostname still checked",
            &["other.example"],
            WinHttpTlsConfig::builder().accept_invalid_certs(true).build(),
        )
        .await?;
        report(
            "untrusted and hostname mismatch both accepted",
            &["other.example"],
            WinHttpTlsConfig::builder()
                .accept_invalid_certs(true)
                .accept_invalid_hostnames(true)
                .build(),
        )
        .await?;
        Ok(())
    }

    async fn report(label: &str, certificate_names: &[&str], tls: WinHttpTlsConfig) -> Result<(), ohno::AppError> {
        let server = TestServer::https([ResponsePlan::ok("secured")], certificate_names);
        let test_client = client(&[Version::HTTP_11], tls, Clock::new_tokio());

        let outcome = match test_client.client.get(server.url("/tls")).fetch_text_body().await {
            Ok(body) => format!("accepted: {body}"),
            Err(error) => format!("rejected: {error}"),
        };

        println!("{label}: {outcome}");
        drop(server.finish());
        Ok(())
    }
}
