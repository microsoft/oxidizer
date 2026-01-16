// Copyright (c) Microsoft Corporation.

//! Advanced retry middleware demonstrating custom input cloning and attempt info forwarding.
//!
//! Shows how to inject attempt metadata into requests via `.clone_input()`, access it
//! in the service function, and forward it through to the final output.

use std::io::Error;
use std::time::Duration;

use http::{Request, Response};
use layered::{Execute, Service, Stack};
use opentelemetry_sdk::metrics::SdkMeterProvider;
use opentelemetry_stdout::MetricExporter;
use seatbelt::retry::Retry;
use seatbelt::{Attempt, RecoveryInfo, SeatbeltOptions};
use tick::Clock;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let meter_provider = configure_telemetry();

    let clock = Clock::new_tokio();
    let options = SeatbeltOptions::new(&clock)
        .pipeline_name("retry_advanced")
        .meter_provider(&meter_provider);

    // Define stack with retry layer
    let stack = (
        Retry::layer("my_retry", &options)
            // Custom input cloning - inject attempt info into request extensions
            .clone_input_with(|input: &mut Request<String>, args| {
                let mut cloned = input.clone();
                cloned.extensions_mut().insert(args.attempt());
                Some(cloned)
            })
            .max_retry_attempts(10)
            .use_jitter(true)
            .base_delay(Duration::from_millis(100))
            .recovery_with(|output, _args| match output {
                Ok(_) => RecoveryInfo::never(),
                Err(_) => RecoveryInfo::retry(),
            })
            // Register a callback called just before the next retry
            .on_retry(|_output, args| {
                println!(
                    "retrying, attempt {}, delay: {}s",
                    args.attempt().index(),
                    args.retry_delay().as_secs_f32(),
                );
            }),
        Execute::new(send_request),
    );

    // Create the service from the stack
    let service = stack.build();

    let request = Request::builder()
        .uri("https://example.com")
        .body("value".to_string())?;

    match service.execute(request).await {
        Ok(output) => {
            // Extract attempt info that was forwarded through the pipeline
            let attempts = output
                .extensions()
                .get::<Attempt>()
                .map_or(0, |a| a.index());
            println!(
                "execution succeeded, result: {}, attempts: {}",
                output.body(),
                attempts
            );
        }
        Err(e) => println!("execution failed, error: {e}"),
    }

    // Flush metrics to stdout before exiting
    meter_provider.force_flush()?;

    Ok(())
}

// Only 20% chance of success, retries will be attempted with a high probability
async fn send_request(input: Request<String>) -> Result<Response<String>, Error> {
    if fastrand::i16(0..10) > 2 {
        Err(Error::other("transient execution error"))
    } else {
        // Extract attempt info that was injected during custom cloning
        let attempt = input
            .extensions()
            .get::<Attempt>()
            .copied()
            .unwrap_or_default();

        // Forward attempt info to output via response extensions
        Response::builder()
            .extension(attempt)
            .body("success".to_string())
            .map_err(Error::other)
    }
}

fn configure_telemetry() -> SdkMeterProvider {
    // Set up tracing subscriber for logs to console
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer())
        .init();

    SdkMeterProvider::builder()
        .with_periodic_exporter(MetricExporter::default())
        .build()
}
