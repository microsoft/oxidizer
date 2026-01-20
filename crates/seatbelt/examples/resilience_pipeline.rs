// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! This example demonstrates how to combine multiple resilience middlewares
//! using the `seatbelt` crate to create a robust execution pipeline with basic
//! resilience capabilities.

use std::time::Duration;

use layered::{Execute, Service, Stack};
use ohno::AppError;
use opentelemetry_sdk::metrics::SdkMeterProvider;
use opentelemetry_stdout::MetricExporter;
use seatbelt::retry::Retry;
use seatbelt::timeout::Timeout;
use seatbelt::{Context, RecoveryInfo};
use tick::Clock;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

#[tokio::main]
async fn main() -> Result<(), AppError> {
    let meter_provider = configure_telemetry();

    let clock = Clock::new_tokio();

    // Shared options for resilience middleware
    let context = Context::new(&clock).enable_metrics(&meter_provider).pipeline_name("my_pipeline");

    // Define stack with retry and timeout middlewares
    let stack = (
        Retry::layer("my_retry", &context)
            // automatically clones the input for retries
            .clone_input()
            // classify the output
            .recovery_with(|output: &String, _args| match output.as_str() {
                "error" | "timeout" => RecoveryInfo::retry(),
                _ => RecoveryInfo::never(),
            }),
        Timeout::layer("my_timeout", &context)
            .timeout(Duration::from_secs(1))
            .timeout_output(|_args| "timeout".to_string()),
        Execute::new(execute_operation),
    );

    // Build the service
    let service = stack.build();

    // Execute the service with an input
    let output = service.execute("value".to_string()).await;

    println!("execution finished, output: {output}");

    // Flush metrics to stdout before exiting
    meter_provider.force_flush()?;

    Ok(())
}

async fn execute_operation(input: String) -> String {
    if fastrand::i16(0..10) > 4 { "error".to_string() } else { input }
}

fn configure_telemetry() -> SdkMeterProvider {
    // Set up tracing subscriber for logs to console
    tracing_subscriber::registry().with(tracing_subscriber::fmt::layer()).init();

    SdkMeterProvider::builder()
        .with_periodic_exporter(MetricExporter::default())
        .build()
}
