// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! This example simulates a major service outage.
//! It uses the [`Injection`] chaos middleware to change the failure rate:
//!
//! 1. Early requests (input ≤ 15) fail at an 80% rate. This opens the circuit quickly.
//! 2. Mid-range requests (input 16–100) fail at a 40% rate. This represents a degraded service.
//! 3. Later requests (input > 100) do not fail. This represents full recovery.
//! 4. The circuit breaker opens when the failure rate exceeds the threshold.
//!    It probes the service and closes when the service recovers.

use std::time::Duration;

use layered::{Execute, Service, Stack};
use ohno::AppError;
use opentelemetry_sdk::metrics::SdkMeterProvider;
use opentelemetry_stdout::MetricExporter;
use seatbelt::breaker::Breaker;
use seatbelt::chaos::injection::Injection;
use seatbelt::{RecoveryInfo, ResilienceContext};
use tick::Clock;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

#[tokio::main]
async fn main() -> Result<(), AppError> {
    let meter_provider = configure_telemetry();

    let clock = Clock::new_tokio();
    let context = ResilienceContext::new(&clock).use_metrics(&meter_provider);

    // Define a stack with circuit breaker and chaos injection layers.
    //
    // The Injection layer is inside the breaker. The breaker sees the inner
    // service fail in the same way that it sees a real downstream outage.
    let stack = (
        Breaker::layer("my_breaker", &context)
            // Classify the output.
            .recovery_with(|output, _args| match output {
                Ok(_) => RecoveryInfo::never(),
                Err(_) => RecoveryInfo::retry(),
            })
            // Provide output while the circuit is open.
            .rejected_input_error(|input, _args| format!("rejecting execution of '{input}' because circuit is open"))
            // Decrease these values to open the circuit sooner.
            .sampling_duration(Duration::from_secs(2))
            .min_throughput(5)
            .break_duration(Duration::from_secs(2))
            .on_probing(|_, _| println!("probing input let in to see if the service has recovered"))
            .on_opened(|_, _| println!("circuit opened due to exceeding failure threshold"))
            .on_closed(|_, args| {
                println!(
                    "circuit closed because probing succeeded, opened for: {}s",
                    args.open_duration().as_secs()
                );
            }),
        // Simulate failures at a rate that decreases over time.
        Injection::layer("simulated_outage", &context)
            .rate_with(|input: &u32, _args| {
                if *input > 100 {
                    // The service recovered. Do not inject failures.
                    0.0
                } else if *input > 15 {
                    // The service is degraded. Use a moderate failure rate.
                    0.4
                } else {
                    // Use a high initial failure rate to open the circuit quickly.
                    0.8
                }
            })
            .output_error_with(|input, _args| format!("transient error for '{input}'")),
        Execute::new(execute_operation),
    );

    // Create the service from the stack.
    let service = stack.into_service();

    // Run several calls. The circuit opens when the failure rate exceeds the threshold.
    // Increase this value to 300 to see the circuit close after the service recovers.
    for attempt in 0..30 {
        clock.delay(Duration::from_millis(50)).await;

        match service.execute(attempt).await {
            Ok(output) => println!("{attempt}: {output}"),
            Err(e) => println!("{attempt}: {e}"),
        }
    }

    // Flush metrics to standard output before the program exits.
    meter_provider.force_flush()?;

    Ok(())
}

// The inner service always succeeds. The Injection layer adds failures.
async fn execute_operation(input: u32) -> Result<String, String> {
    Ok(format!("output-{input}"))
}

fn configure_telemetry() -> SdkMeterProvider {
    // Send tracing logs to the console.
    tracing_subscriber::registry().with(tracing_subscriber::fmt::layer()).init();

    SdkMeterProvider::builder()
        .with_periodic_exporter(MetricExporter::default())
        .build()
}
