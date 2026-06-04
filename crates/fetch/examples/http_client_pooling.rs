// Copyright (c) Microsoft Corporation.

//! # HTTP Client Pooling Example
//!
//! By default, the HTTP client only supports a single HTTP/2 connection per host.
//! This can become a bottleneck when making many concurrent requests to the same endpoint.
//!
//! This example demonstrates how to work around this limitation by using multiple connection
//! pools. Each pool maintains its own HTTP/2 connection, allowing for better parallelism
//! and throughput when making concurrent requests.

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

    println!("example finished");

    Ok(())
}
