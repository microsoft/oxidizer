// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Demonstrates the HTTP circuit breaker with per-origin isolation.
//!
//! A fake handler returns 500 Internal Server Error for `https://failing.example.com` and 200 OK
//! for `https://healthy.example.com`.
//!
//! 1. Requests to the failing host open its circuit breaker.
//! 2. The open breaker rejects another request before it reaches the handler.
//! 3. A request to the healthy host succeeds because breaker state is per origin.

use std::time::Duration;

use fetch::fake::{FakeDeps, FakeHandler};
use fetch::{HttpClient, HttpResponseBuilder};
use http::StatusCode;
use ohno::ErrorExt;

const FAILING_HOST: &str = "https://failing.example.com";
const HEALTHY_HOST: &str = "https://healthy.example.com";

#[tokio::main]
async fn main() -> Result<(), ohno::AppError> {
    // Return 500 for the failing host and 200 for other hosts.
    let fake_handler = FakeHandler::from_fn(|req| {
        let status = if req.uri().host() == Some("failing.example.com") {
            StatusCode::INTERNAL_SERVER_ERROR
        } else {
            StatusCode::OK
        };

        HttpResponseBuilder::new_fake().status(status).build()
    });

    let client = HttpClient::builder_fake(fake_handler, FakeDeps::default())
        .standard_pipeline(|pipeline, _| {
            pipeline
                // Disable retries to observe the breaker directly.
                .retry(|retry| retry.max_retry_attempts(0))
                // Use low thresholds so the breaker opens after this short request sequence.
                .breaker(|breaker| {
                    breaker
                        .min_throughput(5)
                        .failure_threshold(0.5)
                        .break_duration(Duration::from_secs(30))
                })
        })
        .build();

    // ---------------------------------------------------------------
    // Step 1: Open the breaker for the failing host.
    // ---------------------------------------------------------------
    println!("--- Sending requests to {FAILING_HOST}: 500 responses, then breaker errors ---");

    for i in 1..=10 {
        let result = client.get(FAILING_HOST).fetch().await;
        match result {
            Ok(resp) => println!("  request {i}: {}", resp.status()),
            Err(e) => println!("  request {i}: error: {}", e.message()),
        }
    }

    // ---------------------------------------------------------------
    // Step 2: Send a request that the open breaker rejects before the handler.
    // ---------------------------------------------------------------
    println!("\n--- Breaker is open for {FAILING_HOST} ---");

    let result = client.get(FAILING_HOST).fetch().await;
    match result {
        Ok(resp) => println!("  unexpected success: {}", resp.status()),
        Err(e) => println!("  rejected: {}", e.message()),
    }

    // ---------------------------------------------------------------
    // Step 3: Send a request to the healthy origin.
    // ---------------------------------------------------------------
    println!("\n--- Sending request to {HEALTHY_HOST} ---");

    let response = client.get(HEALTHY_HOST).fetch().await?;
    println!("  response: {}", response.status());

    assert_eq!(
        response.status(),
        StatusCode::OK,
        "the failing host breaker must not affect the healthy host"
    );

    println!("\nCircuit breaker isolation between origins works.");

    Ok(())
}
