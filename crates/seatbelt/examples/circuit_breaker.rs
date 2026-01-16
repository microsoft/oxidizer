// Copyright (c) Microsoft Corporation.

//! Circuit breaker example that simulates a major service outage and tripping of the
//! circuit breaker by:
//!
//! 1. Monitoring failure rates in real-time
//! 2. Opening the circuit when failure thresholds are exceeded
//! 3. Allowing probe requests to test service recovery
//! 4. Automatically closing the circuit when the service recovers

use std::time::Duration;

use layered::{Execute, Service, Stack};
use opentelemetry_sdk::metrics::SdkMeterProvider;
use opentelemetry_stdout::MetricExporter;
use seatbelt::circuit_breaker::CircuitBreaker;
use seatbelt::{RecoveryInfo, SeatbeltOptions};
use tick::Clock;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let meter_provider = configure_telemetry();

    let clock = Clock::new_tokio();
    let options = SeatbeltOptions::new(&clock).meter_provider(&meter_provider);

    // Define stack with circuit breaker layer
    let stack = (
        CircuitBreaker::layer("my_circuit_breaker", &options)
            // Required: classify the recoverability of outputs
            .recovery_with(|output, _args| match output {
                Ok(_) => RecoveryInfo::never(),
                Err(_) => RecoveryInfo::retry(),
            })
            // Required: provide output when circuit is open
            .rejected_input_error(|input, _args| {
                format!("rejecting execution of '{input}' because circuit is open")
            })
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
        Execute::new(execute_operation),
    );

    // Create the service from the stack
    let service = stack.build();

    // Execute multiple attempts, the circuit breaker will eventually open because the
    // failure rate exceeds the threshold. You can play with this value an increase it to 300
    // to see how the circuit breaker eventually closes when the service recovers.
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

// Simulate major service outage, 50% chance of failing
async fn execute_operation(input: u32) -> Result<String, String> {
    // After input 100, the service recovers and always succeeds
    if input > 100 {
        return Ok(format!("output-{input}"));
    }

    if fastrand::i16(0..10) > 5 {
        Err(format!("transient error for '{input}'"))
    } else {
        // Produce some output
        Ok(format!("output-{input}"))
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
