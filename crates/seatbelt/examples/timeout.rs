// Copyright (c) Microsoft Corporation.

//! Simple timeout resilience middleware example.
//!
//! This example demonstrates the basic usage of the timeout middleware to cancel
//! long-running operations.

use std::time::Duration;

use anyhow::anyhow;
use layered::{Execute, Service, Stack};
use opentelemetry_sdk::metrics::SdkMeterProvider;
use opentelemetry_stdout::MetricExporter;
use seatbelt::SeatbeltOptions;
use seatbelt::timeout::Timeout;
use tick::Clock;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

const TIMEOUT_DURATION: Duration = Duration::from_millis(100);
const PROCESSING_DELAY: Duration = Duration::from_millis(500);

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let meter_provider = configure_telemetry();

    let clock = Clock::new_tokio();

    // Create common options
    let options = SeatbeltOptions::new(&clock).meter_provider(&meter_provider);

    // Define stack with timeout layer
    let stack = (
        Timeout::layer("my_timeout", &options)
            // Required: specify the timeout duration
            .timeout(TIMEOUT_DURATION)
            // Required: create error output for timeouts
            .timeout_error(|args| {
                anyhow!(
                    "timeout occurred, timeout: {}ms",
                    args.timeout().as_millis()
                )
            }),
        Execute::new({
            let clock = clock.clone();
            move |_input| {
                let clock = clock.clone();
                async move {
                    clock.delay(PROCESSING_DELAY).await; // Simulate some processing delay so the timeout can trigger
                    Ok(())
                }
            }
        }),
    );

    // Create the service from the stack
    let service = stack.build();

    for i in 0..10 {
        // Execute the service, results in a timeout error
        let timeout_error = service.execute(i.to_string()).await.unwrap_err();
        println!("{i} attempt, error: {timeout_error}");
    }

    // Flush metrics to stdout before exiting
    meter_provider.force_flush()?;

    Ok(())
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
