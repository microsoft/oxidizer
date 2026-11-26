// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Sends HTTP requests on Tokio.

use bytesbuf::mem::GlobalPool;
use fetch::HttpClient;
use fetch::tokio::TokioDeps;
use tick::Clock;

#[tokio::main]
async fn main() -> Result<(), ohno::AppError> {
    let client = HttpClient::new_tokio(TokioDeps::new(&Clock::new_tokio(), GlobalPool::new()));

    let response = client.get("https://example.com").fetch().await?;
    println!("Request completed with status: {}", response.status());

    Ok(())
}
