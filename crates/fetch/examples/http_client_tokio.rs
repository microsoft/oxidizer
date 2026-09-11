// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Demonstrates basic usage of the HTTP client on the Tokio runtime.

use fetch::HttpClient;
use fetch::options::DecompressionMethod;
use fetch::tokio::TokioDeps;

#[tokio::main]
async fn main() -> Result<(), ohno::AppError> {
    let client = HttpClient::builder_tokio(TokioDeps::default())
        .response_decompression(DecompressionMethod::ALL)
        .build();

    let response = client.get("https://example.com").fetch().await?;
    println!("Request completed with status: {}", response.status());

    Ok(())
}
