// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! This example demonstrates how to enable native TLS support for the HTTP client.

use fetch::HttpClient;
use fetch::tls::TlsOptions;

#[tokio::main]
async fn main() -> Result<(), ohno::AppError> {
    let client = HttpClient::builder_tokio(fetch::tokio::TokioDeps::default())
        .tls_options(TlsOptions::new_native_tls())
        .build();

    let response = client.get("https://example.com").fetch().await?;

    println!("Request completed with status: {}", response.status());

    Ok(())
}
