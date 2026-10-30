// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Sends HTTP requests on Tokio.

use fetch::HttpClient;

#[tokio::main]
async fn main() -> Result<(), ohno::AppError> {
    let client = HttpClient::new_tokio();

    let response = client.get("https://example.com").fetch().await?;
    println!("Request completed with status: {}", response.status());

    Ok(())
}
