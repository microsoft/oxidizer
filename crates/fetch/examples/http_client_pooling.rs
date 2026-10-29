// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! # HTTP client pooling
//!
//! Creates 10 fake transport slots and selects them in round-robin order. The example sends 1,000
//! sequential requests. It exercises pool-slot selection without network I/O.

use fetch::HttpClient;
use fetch::fake::FakeDeps;
use fetch::options::{ConnectionPoolOptions, PoolSelection};
use http::StatusCode;

#[path = "util/utils.rs"]
mod utils;

#[tokio::main]
async fn main() -> Result<(), ohno::AppError> {
    let client = HttpClient::builder_fake(StatusCode::OK, FakeDeps::default())
        .minimal_pipeline()
        .connection_pool_options(ConnectionPoolOptions::default().multiple_pools(10, PoolSelection::round_robin()))
        .build();

    for _ in 0..1000 {
        _ = client.get("https://example.com").fetch().await?;
    }

    println!("Example is complete.");

    Ok(())
}
