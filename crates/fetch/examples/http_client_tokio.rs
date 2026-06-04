// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Demonstrates basic usage of the HTTP client on the Tokio runtime.

use fetch::HttpClient;

#[tokio::main]
async fn main() -> Result<(), ohno::AppError> {
    let client = HttpClient::new_tokio();

    let response = client.get("https://example.com").fetch().await?;
    println!("Request completed with status: {}", response.status());

    Ok(())
}
