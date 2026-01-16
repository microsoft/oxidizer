// Copyright (c) Microsoft Corporation.

//! This example demonstrates how to combine multiple resilience middlewares
//! using the `seatbelt` crate to create a robust execution pipeline with basic
//! resilience capabilities.

use std::time::Duration;

use layered::{Execute, Service, Stack};
use oxidizer_rt::Builtins;
use oxidizer_telemetry::Telemetry;
use oxidizer_telemetry::destination::{stdout_logs, stdout_metrics};
use oxidizer_telemetry::logs::InterceptTracingToLogExporter;
use seatbelt::retry::Retry;
use seatbelt::timeout::Timeout;
use seatbelt::{RecoveryInfo, SeatbeltOptions};
use tracing_subscriber::util::SubscriberInitExt;

#[oxidizer_rt::main]
async fn main(builtins: Builtins) {
    let telemetry = configure_telemetry();

    // Shared options for resilience middleware
    let options = SeatbeltOptions::new(&builtins)
        .meter_provider(telemetry.meter_provider())
        .pipeline_name("my_pipeline");

    // Define stack with retry and timeout middlewares
    let stack = (
        Retry::layer("my_retry", &options)
            // automatically clones the input for retries
            .clone_input()
            // classify the output
            .recovery_with(|output: &String, _args| match output.as_str() {
                "error" | "timeout" => RecoveryInfo::retry(),
                _ => RecoveryInfo::never(),
            }),
        Timeout::layer("my_timeout", &options)
            .timeout(Duration::from_secs(1))
            .timeout_output(|_args| "timeout".to_string()),
        Execute::new(execute_operation),
    );

    // Build the service
    let service = stack.build();

    // Execute the service with an input
    let output = service.execute("value".to_string()).await;

    println!("execution finished, output: {output}");
}

async fn execute_operation(input: String) -> String {
    if fastrand::i16(0..10) > 4 {
        "error".to_string()
    } else {
        input
    }
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
