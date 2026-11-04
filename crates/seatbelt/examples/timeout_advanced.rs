// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! This example uses advanced timeout middleware features.
//!
//! It uses `Result` output, timeout callbacks, and timeouts based on the input.

use std::time::Duration;

use layered::{Execute, Service, Stack};
use ohno::{AppError, app_err};
use opentelemetry_sdk::metrics::SdkMeterProvider;
use opentelemetry_stdout::MetricExporter;
use seatbelt::ResilienceContext;
use seatbelt::timeout::Timeout;
use tick::Clock;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

const TIMEOUT_DURATION: Duration = Duration::from_millis(20);
const PROCESSING_DELAY: Duration = Duration::from_secs(1);

#[tokio::main]
async fn main() -> Result<(), AppError> {
    // Enable timeout metrics and logs.
    let meter_provider = configure_telemetry();

    let clock = Clock::new_tokio();

    // Create the service configuration.
    let context: ResilienceContext<String, Result<(), AppError>> =
        ResilienceContext::new(&clock).name("my_pipeline").use_metrics(&meter_provider);

    // Define a stack with a timeout layer.
    let stack = (
        Timeout::layer("my_timeout", &context)
            // Set the timeout duration.
            .timeout(TIMEOUT_DURATION)
            // Create the error output for a timeout.
            .timeout_error(|args| app_err!("timeout occurred, timeout: {}ms", args.timeout().as_millis()))
            // Run a callback when a timeout occurs.
            .on_timeout(|_out, args| {
                println!("timeout occurred, timeout: {}ms", args.timeout().as_millis());
            })
            // Use the input to override the default timeout.
            .timeout_override(|input, _args| match input.as_str() {
                "2" => Some(Duration::from_millis(300)),
                _ => None,
            }),
        Execute::new({
            let clock = clock.clone();
            move |_input| {
                let clock = clock.clone();
                async move {
                    // Add enough processing delay to trigger the timeout.
                    clock.delay(PROCESSING_DELAY).await;
                    Ok(())
                }
            }
        }),
    );

    // Create the service from the stack.
    let service = stack.into_service();

    for i in 0..10 {
        // Execute the service. Some calls return a timeout error.
        match service.execute(i.to_string()).await {
            Ok(()) => println!("execute, input: {i}, result: success"),
            Err(e) => println!("execute, input: {i}, error: {e}"),
        }
    }

    // Flush metrics to standard output before the program exits.
    meter_provider.force_flush()?;

    Ok(())
}

fn configure_telemetry() -> SdkMeterProvider {
    // Send tracing logs to the console.
    tracing_subscriber::registry().with(tracing_subscriber::fmt::layer()).init();

    SdkMeterProvider::builder()
        .with_periodic_exporter(MetricExporter::default())
        .build()
}
