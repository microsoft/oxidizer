// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![expect(clippy::unwrap_used, reason = "sample code")]

//! This example restores retry input from errors.
//!
//! It handles these outage conditions:
//! - The request body is too expensive to clone.
//! - `restore_input_on_error()` restores input from an error.
//! - A failed request retries against a fallback endpoint.

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

    // Configure retry middleware to restore input after an outage.
    let stack = (
        Retry::layer("outage_retry", &context)
            // Do not clone input. Restore it from the error.
            .clone_input_with(|_, _| None)
            // Use the error type to classify recovery.
            .recovery_with(|output: &Result<_, HttpError>, _| match output {
                Ok(_) => RecoveryInfo::never(), // Do not retry successful responses.
                Err(error) => error.recovery(), // Use the recovery rule from the error.
            })
            // Retry unavailable output.
            .handle_unavailable(true)
            // Restore input from the error before a retry.
            .restore_input_from_error(|error: &mut HttpError, _| {
                // Extract the request and select the fallback endpoint.
                error.try_restore_request()
            }),
        Execute::new(send_request),
    );

    // Create the service from the stack.
    let service = stack.into_service();

    // Create a request that fails first and can be restored.
    let request = Request::builder()
        .uri(ENDPOINT_WITH_OUTAGE)
        .body("important request data".to_string())?;

    println!("Sending request to: {}", request.uri());

    // The service first calls the unavailable endpoint.
    // It restores the request, selects the fallback endpoint, and retries.
    let response = service.execute(request).await?;

    println!("Final response: {}", response.body());

    // Flush metrics to standard output before the program exits.
    meter_provider.force_flush()?;

    Ok(())
}

/// Simulates a failed primary endpoint and a working fallback endpoint.
///
/// The error stores the original request.
/// The retry callback changes its endpoint before the next call.
async fn send_request(input: Request<String>) -> Result<Response<String>, HttpError> {
    if input.uri() == ENDPOINT_WITH_OUTAGE {
        println!("Request to {} failed - simulating outage", input.uri());
        // Store the request in the error for restoration.
        Err(HttpError::outage(input))
    } else {
        println!("Request to {} succeeded", input.uri());
        Ok(Response::new(format!("Success! Data from {}", input.uri())))
    }
}

/// Stores the failed request so that retry middleware can restore it.
///
/// The request can use another endpoint without implementing `Clone`.
#[ohno::error]
struct HttpError {
    /// The failed request for input restoration.
    rejected_request: Option<Box<Request<String>>>,
    /// The recovery rule for this error.
    recovery: RecoveryInfo,
}

impl HttpError {
    /// Creates an outage error that stores the request.
    fn outage(rejected_request: Request<String>) -> Self {
        Self::caused_by(
            Some(Box::new(rejected_request)),
            RecoveryInfo::unavailable().delay(Duration::from_millis(100)),
            "simulated outage",
        )
    }

    /// Restores the request and selects the fallback endpoint.
    ///
    /// `restore_input_on_error()` calls this method before a retry.
    fn try_restore_request(&mut self) -> Option<Request<String>> {
        self.rejected_request
            .take() // Take the stored request.
            .map(|boxed_request| *boxed_request) // Remove the box.
            .map(|mut request| {
                // Select the fallback endpoint.
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
    // Send tracing logs to the console.
    tracing_subscriber::registry().with(tracing_subscriber::fmt::layer()).init();

    SdkMeterProvider::builder()
        .with_periodic_exporter(MetricExporter::default())
        .build()
}
