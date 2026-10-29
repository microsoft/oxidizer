// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Sends concurrent requests to example.com. The pool can add connections based on the negotiated
//! HTTP version and the server limits.

use std::time::Duration;

use fetch::HttpClient;
use fetch::options::ConnectionPoolOptions;
use fetch::tokio::TokioDeps;
use tokio::spawn;
use tracing::info;

#[path = "util/endpoint.rs"]
mod endpoint;
#[path = "util/utils.rs"]
mod utils;

const CONCURRENT_REQUESTS: usize = 20;
const URL: &str = "https://example.com";

#[tokio::main]
async fn main() -> Result<(), ohno::AppError> {
    utils::init_tracing();
    let endpoint = endpoint::Endpoint::new(URL).await?;

    let deps = TokioDeps::default();
    let clock = deps.clock.clone();

    let client = HttpClient::builder_tokio(deps)
        .tls_options(endpoint.rustls_options()?)
        .connection_pool_options(ConnectionPoolOptions::default().connection_idle_timeout(Duration::from_secs(5)))
        .build();

    let mut handles = vec![];

    // This request burst can create more connections, depending on protocol and server limits.
    for _ in 0..CONCURRENT_REQUESTS {
        let client_clone = client.clone();
        let url = endpoint.url.clone();
        let handle = spawn(async move { client_clone.get(url.as_str()).fetch_text().await });

        handles.push(handle);
    }

    // Wait for every request.
    for handle in handles {
        handle.await??;
    }

    info!(
        "Sent {} concurrent requests. The application stops after 10 seconds. \
    Inspect the logs for idle connection closures.",
        CONCURRENT_REQUESTS
    );

    clock.delay(Duration::from_secs(10)).await;

    endpoint.finish(CONCURRENT_REQUESTS).await?;
    Ok(())
}
