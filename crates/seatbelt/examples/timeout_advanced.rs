// Copyright (c) Microsoft Corporation.

//! Advanced timeout resilience middleware example.
//!
//! This example demonstrates advanced usage of the timeout middleware, including working with
//! Result-based outputs, timeout callbacks, and dynamic timeout durations based on input.

use std::time::Duration;

use anyhow::anyhow;
use layered::{Execute, Service, Stack};
use oxidizer_rt::Builtins;
use oxidizer_telemetry::Telemetry;
use oxidizer_telemetry::destination::{stdout_logs, stdout_metrics};
use oxidizer_telemetry::logs::InterceptTracingToLogExporter;
use seatbelt::SeatbeltOptions;
use seatbelt::timeout::Timeout;
use tracing_subscriber::prelude::*;

const TIMEOUT_DURATION: Duration = Duration::from_millis(20);
const PROCESSING_DELAY: Duration = Duration::from_secs(1);

#[oxidizer_rt::main]
async fn main(state: Builtins) -> anyhow::Result<()> {
    // Configure telemetry to see the timeout metrics and logs
    let telemetry = configure_telemetry();

    // Create service options
    let options: SeatbeltOptions<String, anyhow::Result<()>> = SeatbeltOptions::new(&state)
        .pipeline_name("my_pipeline")
        .meter_provider(telemetry.meter_provider());

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
            })
            // Optional: callback to be invoked when a timeout occurs
            .on_timeout(|_out, args| {
                println!(
                    "timeout occurred, timeout: {}ms",
                    args.timeout().as_millis()
                );
            })
            // Optional: override the default timeout duration by inspecting the input
            .timeout_override(|input, _args| match input.as_str() {
                "2" => Some(Duration::from_millis(300)),
                _ => None,
            }),
        Execute::new(move |_input| {
            let clock = state.clock().clone();
            async move {
                // Simulate some processing delay so the timeout can trigger
                clock.delay(PROCESSING_DELAY).await;
                Ok(())
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

    Ok(())
}

fn configure_telemetry() -> Telemetry {
    let telemetry = oxidizer_telemetry::builder()
        .destination(stdout_metrics())
        .unwrap()
        .destination(stdout_logs())
        .unwrap()
        .build();

    tracing_subscriber::registry()
        .with_tracing_interception(telemetry.logger_provider())
        .init();

    telemetry
}
