// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Demonstrates how to build a resilience pipeline on top of an arbitrary HTTP client
//! and encapsulate it behind a clean `MyClient` API using [`DynamicService`] for type erasure.
//!
//! Pipeline (outermost to innermost):
//!
//! 1. **Outer timeout** – caps the total wall-clock time for the entire operation,
//!    including all retry attempts.
//! 2. **Retry** – retries transient failures (connection errors, HTTP 5xx responses).
//! 3. **Inner timeout** – caps the time for each individual HTTP request attempt.
//! 4. **reqwest** – performs the actual HTTP request.

use std::io::{Error, ErrorKind};
use std::time::Duration;

use layered::{DynamicService, DynamicServiceExt, Execute, Service, Stack};
use seatbelt::retry::Retry;
use seatbelt::timeout::Timeout;
use seatbelt::{RecoveryInfo, ResilienceContext};
use tick::Clock;

// ---------------------------------------------------------------------------
// Request / Response types
// ---------------------------------------------------------------------------

/// A simple HTTP request description.
#[derive(Clone, Debug)]
struct Request {
    url: String,
}

/// The HTTP response returned by [`MyClient`].
#[derive(Debug)]
struct Response {
    status: u16,
    body: String,
}

// ---------------------------------------------------------------------------
// MyClient – resilience-wrapped HTTP client
// ---------------------------------------------------------------------------

/// An HTTP client that transparently applies timeout and retry policies.
///
/// Internally the middleware stack is type-erased into a [`DynamicService`] so
/// callers never see the complex composed type.
#[derive(Debug, Clone)]
struct MyClient {
    service: DynamicService<Request, Result<Response, Error>>,
}

impl MyClient {
    fn new(clock: &Clock) -> Self {
        let context = ResilienceContext::new(clock).name("http_pipeline");
        let client = reqwest::Client::new();

        // Pipeline: outer timeout -> retry -> inner timeout -> reqwest
        let service = (
            // Outer timeout: caps total operation time including all retries.
            Timeout::layer("overall_timeout", &context)
                .timeout(Duration::from_secs(30))
                .timeout_error(|_args| Error::new(ErrorKind::TimedOut, "overall request timeout exceeded")),
            // Retry: retries on transient failures with exponential backoff.
            Retry::layer("retry", &context)
                .clone_input()
                .recovery_with(|output: &Result<Response, Error>, _args| match output {
                    Ok(_) => RecoveryInfo::never(),
                    Err(_) => RecoveryInfo::retry(),
                }),
            // Inner timeout: caps each individual HTTP request attempt.
            Timeout::layer("request_timeout", &context)
                .timeout(Duration::from_secs(5))
                .timeout_error(|_args| Error::new(ErrorKind::TimedOut, "individual request timeout exceeded")),
            // The innermost layer: sends the actual HTTP request via reqwest.
            Execute::new(move |req: Request| {
                let client = client.clone();
                async move {
                    let response = client
                        .get(&req.url)
                        .send()
                        .await
                        .map_err(|e| Error::other(format!("request failed: {e}")))?;

                    let status = response.status().as_u16();
                    if response.status().is_server_error() {
                        return Err(Error::other(format!("server error: {status}")));
                    }

                    let body = response
                        .text()
                        .await
                        .map_err(|e| Error::other(format!("failed to read body: {e}")))?;

                    Ok(Response { status, body })
                }
            }),
        )
            .into_service()
            .into_dynamic();

        Self { service }
    }

    async fn get(&self, url: impl Into<String>) -> Result<Response, Error> {
        self.service.execute(Request { url: url.into() }).await
    }
}

// ---------------------------------------------------------------------------
// main
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let clock = Clock::new_tokio();
    let client = MyClient::new(&clock);

    match client.get("https://example.com").await {
        Ok(resp) => println!("HTTP {}, body length: {} bytes", resp.status, resp.body.len()),
        Err(e) => println!("request failed: {e}"),
    }

    Ok(())
}
