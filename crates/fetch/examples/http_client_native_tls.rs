// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Enables native TLS for the HTTP client.

use fetch::HttpClient;

#[path = "util/endpoint.rs"]
mod endpoint;

#[tokio::main]
async fn main() -> Result<(), ohno::AppError> {
    let endpoint = endpoint::Endpoint::new("https://example.com").await?;
    let client = HttpClient::builder_tokio(fetch::tokio::TokioDeps::default())
        .tls_options(endpoint.native_tls_options()?)
        .build();

    let response = client.get(endpoint.url.as_str()).fetch().await?;

    println!("Request completed with status: {}", response.status());

    endpoint.finish(1).await?;
    Ok(())
}
