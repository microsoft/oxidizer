// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Advanced timeout resilience middleware example.
//!
//! This example demonstrates advanced usage of the timeout middleware, including working with
//! Result-based outputs, timeout callbacks, and dynamic timeout durations based on input.

use std::time::Duration;

use layered::{Execute, Service, Stack};
use ohno::{AppError, app_err};
use opentelemetry_sdk::metrics::SdkMeterProvider;
use opentelemetry_stdout::MetricExporter;
use seatbelt::Context;
use seatbelt::timeout::Timeout;
use tick::Clock;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

const TIMEOUT_DURATION: Duration = Duration::from_millis(20);
const PROCESSING_DELAY: Duration = Duration::from_secs(1);

#[tokio::main]
async fn main() -> Result<(), AppError> {
    // Configure telemetry to see the timeout metrics and logs
    let meter_provider = configure_telemetry();

    let clock = Clock::new_tokio();

    // Create service options
    let context: Context<String, Result<(), AppError>> = Context::new(&clock).pipeline_name("my_pipeline").enable_metrics(&meter_provider);

    // Define stack with timeout layer
    let stack = (
        Timeout::layer("my_timeout", &context)
            // Required: specify the timeout duration
            .timeout(TIMEOUT_DURATION)
            // Required: create error output for timeouts
            .timeout_error(|args| app_err!("timeout occurred, timeout: {}ms", args.timeout().as_millis()))
            // Optional: callback to be invoked when a timeout occurs
            .on_timeout(|_out, args| {
                println!("timeout occurred, timeout: {}ms", args.timeout().as_millis());
            })
            // Optional: override the default timeout duration by inspecting the input
            .timeout_override(|input, _args| match input.as_str() {
                "2" => Some(Duration::from_millis(300)),
                _ => None,
            }),
        Execute::new({
            let clock = clock.clone();
            move |_input| {
                let clock = clock.clone();
                async move {
                    // Simulate some processing delay so the timeout can trigger
                    clock.delay(PROCESSING_DELAY).await;
                    Ok(())
                }
            }
        }),
    );

    // Create the service from the stack
    let service = stack.build();

    for i in 0..10 {
        // Execute the service, results in a timeout error
        match service.execute(i.to_string()).await {
            Ok(()) => println!("execute, input: {i}, result: success"),
            Err(e) => println!("execute, input: {i}, error: {e}"),
        }
    }

    // Flush metrics to stdout before exiting
    meter_provider.force_flush()?;

    Ok(())
}

fn configure_telemetry() -> SdkMeterProvider {
    // Set up tracing subscriber for logs to console
    tracing_subscriber::registry().with(tracing_subscriber::fmt::layer()).init();

    SdkMeterProvider::builder()
        .with_periodic_exporter(MetricExporter::default())
        .build()
}
