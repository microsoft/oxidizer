// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Circuit breaker example that simulates a major service outage and tripping of the
//! circuit breaker by leveraging the [`Injection`] chaos middleware to inject failures
//! with a dynamic rate:
//!
//! 1. Early requests (input ≤ 15) fail at 80% to quickly trip the breaker
//! 2. Mid-range requests (input 16–100) fail at 40%, simulating a degraded service
//! 3. Later requests (input > 100) never fail, simulating full recovery
//! 4. The circuit breaker monitors failure rates, opens when thresholds are exceeded,
//!    probes the service to detect recovery, and closes automatically

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

    // Define stack with circuit breaker + chaos injection layers.
    //
    // The Injection layer sits inside the breaker so that from the breaker's
    // perspective the inner service is "failing" — exactly as a real downstream
    // outage would look.
    let stack = (
        Breaker::layer("my_breaker", &context)
            // Required: classify the recoverability of outputs
            .recovery_with(|output, _args| match output {
                Ok(_) => RecoveryInfo::never(),
                Err(_) => RecoveryInfo::retry(),
            })
            // Required: provide output when circuit is open
            .rejected_input_error(|input, _args| format!("rejecting execution of '{input}' because circuit is open"))
            // Decrease the following values to see the circuit breaker trip faster
            // and speed-up the example
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
        // Chaos injection layer: simulate failures with a dynamic rate that
        // decreases over time so the service eventually "recovers".
        Injection::layer("simulated_outage", &context)
            .rate_with(|input: &mut u32, _args| {
                if *input > 100 {
                    // Service has fully recovered — no injected failures
                    0.0
                } else if *input > 15 {
                    // Degraded service — moderate failure rate
                    0.4
                } else {
                    // Initial burst of failures — high rate to trip the breaker quickly
                    0.8
                }
            })
            .output_error_with(|input, _args| format!("transient error for '{input}'")),
        Execute::new(execute_operation),
    );

    // Create the service from the stack
    let service = stack.into_service();

    // Execute multiple attempts, the circuit breaker will eventually open because the
    // failure rate exceeds the threshold. You can play with this value and increase it
    // to 300 to see how the circuit breaker eventually closes when the service recovers.
    for attempt in 0..30 {
        clock.delay(Duration::from_millis(50)).await;

        match service.execute(attempt).await {
            Ok(output) => println!("{attempt}: {output}"),
            Err(e) => println!("{attempt}: {e}"),
        }
    }

    // Flush metrics to stdout before exiting
    meter_provider.force_flush()?;

    Ok(())
}

// The inner service always succeeds — failures are injected by the Injection layer.
async fn execute_operation(input: u32) -> Result<String, String> {
    Ok(format!("output-{input}"))
}

fn configure_telemetry() -> SdkMeterProvider {
    // Set up tracing subscriber for logs to console
    tracing_subscriber::registry().with(tracing_subscriber::fmt::layer()).init();

    SdkMeterProvider::builder()
        .with_periodic_exporter(MetricExporter::default())
        .build()
}
