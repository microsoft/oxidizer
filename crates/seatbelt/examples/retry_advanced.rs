// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! This example uses custom input cloning and passes attempt data through the pipeline.
//!
//! It adds attempt data to requests with `.clone_input()`.
//! The service reads the data and adds it to the final output.

use std::io::Error;
use std::time::Duration;

use http::{Request, Response};
use layered::{Execute, Service, Stack};
use ohno::AppError;
use opentelemetry_sdk::metrics::SdkMeterProvider;
use opentelemetry_stdout::MetricExporter;
use seatbelt::retry::Retry;
use seatbelt::{Attempt, RecoveryInfo, ResilienceContext};
use tick::Clock;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

#[tokio::main]
async fn main() -> Result<(), AppError> {
    let meter_provider = configure_telemetry();

    let clock = Clock::new_tokio();
    let context = ResilienceContext::new(&clock).name("retry_advanced").use_metrics(&meter_provider);

    // Define a stack with a retry layer.
    let stack = (
        Retry::layer("my_retry", &context)
            // Clone the input and add attempt data to the request extensions.
            .clone_input_with(|input: &mut Request<String>, args| {
                let mut cloned = input.clone();
                cloned.extensions_mut().insert(args.attempt());
                Some(cloned)
            })
            .max_retry_attempts(5)
            .use_jitter(true)
            .base_delay(Duration::from_millis(50))
            .recovery_with(|output, _args| match output {
                Ok(_) => RecoveryInfo::never(),
                Err(_) => RecoveryInfo::retry(),
            })
            // Run a callback before the next retry.
            .on_retry(|_output, args| {
                println!(
                    "retrying, attempt {}, delay: {}s",
                    args.attempt().index(),
                    args.retry_delay().as_secs_f32(),
                );
            }),
        Execute::new(send_request),
    );

    // Create the service from the stack.
    let service = stack.into_service();

    let request = Request::builder().uri("https://example.com").body("value".to_string())?;

    match service.execute(request).await {
        Ok(output) => {
            // Read the attempt data from the pipeline.
            let attempts = output.extensions().get::<Attempt>().map_or(0, |a| a.index());
            println!("execution succeeded, result: {}, attempts: {}", output.body(), attempts);
        }
        Err(e) => println!("execution failed, error: {e}"),
    }

    // Flush metrics to standard output before the program exits.
    meter_provider.force_flush()?;

    Ok(())
}

// Succeed for 30% of calls. Most failures cause a retry.
async fn send_request(input: Request<String>) -> Result<Response<String>, Error> {
    if fastrand::i16(0..10) > 2 {
        Err(Error::other("transient execution error"))
    } else {
        // Read the attempt data that the clone callback added.
        let attempt = input.extensions().get::<Attempt>().copied().unwrap_or_default();

        // Add the attempt data to the response extensions.
        Response::builder()
            .extension(attempt)
            .body("success".to_string())
            .map_err(Error::other)
    }
}

fn configure_telemetry() -> SdkMeterProvider {
    // Send tracing logs to the console.
    tracing_subscriber::registry().with(tracing_subscriber::fmt::layer()).init();

    SdkMeterProvider::builder()
        .with_periodic_exporter(MetricExporter::default())
        .build()
}
