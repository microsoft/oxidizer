// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![expect(clippy::unwrap_used, reason = "sample code")]

//! Demonstrates advanced retry patterns with input restoration from errors.
//!
//! This example showcases how to handle outage scenarios where:
//! - The original input cannot be cloned (expensive request bodies)
//! - Input must be restored from error information using `restore_input_on_error()`
//! - Failed requests are automatically retried with a fallback endpoint
//! - Outage detection and recovery are handled seamlessly

use std::time::Duration;

use http::{Request, Response};
use layered::{Execute, Service, Stack};
use ohno::AppError;
use opentelemetry_sdk::metrics::SdkMeterProvider;
use opentelemetry_stdout::MetricExporter;
use seatbelt::retry::Retry;
use seatbelt::{Recovery, RecoveryInfo, ResilienceContext};
use tick::Clock;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

const ENDPOINT_WITH_OUTAGE: &str = "https://example.com";
const ENDPOINT_ALIVE: &str = "https://fallback.example.com";

#[tokio::main]
async fn main() -> Result<(), AppError> {
    let meter_provider = configure_telemetry();

    let clock = Clock::new_tokio();
    let context = ResilienceContext::new(&clock).use_metrics(&meter_provider);

    // Configure retry layer for outage handling with input restoration
    let stack = (
        Retry::layer("outage_retry", &context)
            // Disable input cloning - we'll restore from error instead
            .clone_input_with(|_, _| None)
            // Configure recovery based on an error type
            .recovery_with(|output: &Result<_, HttpError>, _| match output {
                Ok(_) => RecoveryInfo::never(), // Don't retry successful responses
                Err(error) => error.recovery(), // Use error's recovery strategy
            })
            // Enable unavailable detection and handling
            .handle_unavailable(true)
            // Restore input from error when retrying (key feature!)
            .restore_input_from_error(|error: &mut HttpError, _| {
                // Extract the original request and modify it for fallback endpoint
                error.try_restore_request()
            }),
        Execute::new(send_request),
    );

    // Create the service from the stack
    let service = stack.into_service();

    // Create a request that will initially fail but can be restored
    let request = Request::builder()
        .uri(ENDPOINT_WITH_OUTAGE)
        .body("important request data".to_string())?;

    println!("Sending request to: {}", request.uri());

    // The service will:
    // 1. Try the original endpoint (fails with outage)
    // 2. Restore input from the error with fallback endpoint
    // 3. Retry with the modified request (succeeds)
    let response = service.execute(request).await?;

    println!("Final response: {}", response.body());

    // Flush metrics to stdout before exiting
    meter_provider.force_flush()?;

    Ok(())
}

/// Simulates a service that has outages on the primary endpoint but works on fallback.
///
/// This demonstrates the input restoration pattern where the original request is preserved
/// in the error so it can be modified and retried against a different endpoint.
async fn send_request(input: Request<String>) -> Result<Response<String>, HttpError> {
    if input.uri() == ENDPOINT_WITH_OUTAGE {
        println!("Request to {} failed - simulating outage", input.uri());
        // Store the original request in the error for later restoration
        Err(HttpError::outage(input))
    } else {
        println!("Request to {} succeeded", input.uri());
        Ok(Response::new(format!("Success! Data from {}", input.uri())))
    }
}

/// Custom error type that preserves the original request for restoration.
///
/// This pattern allows failed requests to be modified and retried against different
/// endpoints without requiring the original input to be cloneable.
#[ohno::error]
struct HttpError {
    /// The original request that failed, preserved for input restoration
    rejected_request: Option<Box<Request<String>>>,
    /// Recovery strategy (retry vs. never) for this error type
    recovery: RecoveryInfo,
}

impl HttpError {
    /// Creates an outage error that preserves the original request for retry.
    fn outage(rejected_request: Request<String>) -> Self {
        Self::caused_by(
            Some(Box::new(rejected_request)),
            RecoveryInfo::unavailable().delay(Duration::from_millis(100)),
            "simulated outage",
        )
    }

    /// Restores the original request with a modified endpoint for retry.
    ///
    /// This is called by `restore_input_on_error()` to extract and modify the
    /// original request. It changes the URI to the fallback endpoint and returns
    /// the modified request for the next retry attempt.
    fn try_restore_request(&mut self) -> Option<Request<String>> {
        self.rejected_request
            .take() // Extract the stored request
            .map(|boxed_request| *boxed_request) // Unbox it
            .map(|mut request| {
                // Modify the request to use the fallback endpoint
                *request.uri_mut() = ENDPOINT_ALIVE.parse().unwrap();
                println!("Restored request with fallback endpoint: {}", request.uri());
                request
            })
    }
}

impl Recovery for HttpError {
    fn recovery(&self) -> RecoveryInfo {
        self.recovery.clone()
    }
}

fn configure_telemetry() -> SdkMeterProvider {
    // Set up tracing subscriber for logs to console
    tracing_subscriber::registry().with(tracing_subscriber::fmt::layer()).init();

    SdkMeterProvider::builder()
        .with_periodic_exporter(MetricExporter::default())
        .build()
}
