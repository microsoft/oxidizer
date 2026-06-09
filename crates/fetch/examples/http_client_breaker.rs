// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Demonstrates the HTTP circuit breaker with per-origin isolation.
//!
//! This example uses a fake handler that returns 500 Internal Server Error for
//! requests to `https://failing.example.com` and 200 OK for requests to
//! `https://healthy.example.com`.
//!
//! 1. We send many requests to the failing host to trip its circuit breaker.
//! 2. Once the breaker is open, requests to the failing host are rejected
//!    immediately with a "circuit breaker open" error.
//! 3. Requests to the healthy host continue to succeed, demonstrating that
//!    breaker state is tracked per-origin (scheme + authority).

use std::time::Duration;

use fetch::fake::{FakeDeps, FakeHandler};
use fetch::{HttpClient, HttpResponseBuilder};
use http::StatusCode;
use ohno::ErrorExt;

const FAILING_HOST: &str = "https://failing.example.com";
const HEALTHY_HOST: &str = "https://healthy.example.com";

#[tokio::main]
async fn main() -> Result<(), ohno::AppError> {
    // Fake handler: 500 for the failing host, 200 for everything else.
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
                // Disable retries so we can observe the breaker directly.
                .retry(|retry| retry.max_retry_attempts(0))
                // Lower thresholds so the breaker trips quickly in this demo.
                .breaker(|breaker| {
                    breaker
                        .min_throughput(5)
                        .failure_threshold(0.5)
                        .break_duration(Duration::from_secs(30))
                })
        })
        .build();

    // ---------------------------------------------------------------
    // Step 1: Send requests to the failing host to trip the breaker.
    // ---------------------------------------------------------------
    println!("--- Sending requests to {FAILING_HOST} (expect 500s) ---");

    for i in 1..=10 {
        let result = client.get(FAILING_HOST).fetch().await;
        match result {
            Ok(resp) => println!("  request {i}: {}", resp.status()),
            Err(e) => println!("  request {i}: error — {}", e.message()),
        }
    }

    // ---------------------------------------------------------------
    // Step 2: The breaker should now be open — next request is rejected
    //         immediately without reaching the handler.
    // ---------------------------------------------------------------
    println!("\n--- Breaker should be open for {FAILING_HOST} ---");

    let result = client.get(FAILING_HOST).fetch().await;
    match result {
        Ok(resp) => println!("  unexpected success: {}", resp.status()),
        Err(e) => println!("  rejected: {}", e.message()),
    }

    // ---------------------------------------------------------------
    // Step 3: Requests to the healthy host still succeed — the breaker
    //         for a different origin is independent.
    // ---------------------------------------------------------------
    println!("\n--- Sending request to {HEALTHY_HOST} (should succeed) ---");

    let response = client.get(HEALTHY_HOST).fetch().await?;
    println!("  response: {}", response.status());

    assert_eq!(
        response.status(),
        StatusCode::OK,
        "healthy host should not be affected by the failing host's breaker"
    );

    println!("\nDone — circuit breaker isolation between origins is working.");

    Ok(())
}
