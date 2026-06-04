// Copyright (c) Microsoft Corporation.

//! This example demonstrates how to create a simple HTTP client that sends multiple
//! concurrent requests to example.com and how the underlying connection pool dynamically
//! creates new connections to handle the burst of requests.

use std::time::Duration;

use fetch::HttpClient;
use fetch::options::ConnectionPoolOptions;
use fetch::tokio::TokioDeps;
use tokio::spawn;
use tracing::info;

#[path = "util/utils.rs"]
mod utils;

const CONCURRENT_REQUESTS: usize = 20;
const URL: &str = "https://example.com";

#[tokio::main]
async fn main() -> Result<(), ohno::AppError> {
    utils::init_tracing();

    let deps = TokioDeps::default();
    let clock = deps.clock.clone();

    let client = HttpClient::builder_tokio(deps)
        .connection_pool_options(ConnectionPoolOptions::default().connection_idle_timeout(Duration::from_secs(5)))
        .build();

    let mut handles = vec![];

    // This burst of requests creates additional connections.
    for _ in 0..CONCURRENT_REQUESTS {
        let client_clone = client.clone();
        let handle = spawn(async move { client_clone.get(URL).fetch_text().await });

        handles.push(handle);
    }

    // Wait for all requests to complete.
    for handle in handles {
        handle.await??;
    }

    info!(
        "sent {} concurrent requests, the application will automatically stop after 10 seconds. \
    In the logs you should see connections automatically closing after not being used anymore.",
        CONCURRENT_REQUESTS
    );

    clock.delay(Duration::from_secs(10)).await;

    Ok(())
}
