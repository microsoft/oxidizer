// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.
#![expect(clippy::unwrap_used, reason = "sample code")]

//! This example uses timeout middleware.
//!
//! It cancels operations that take too long.

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

const TIMEOUT_DURATION: Duration = Duration::from_millis(100);
const PROCESSING_DELAY: Duration = Duration::from_millis(500);

#[tokio::main]
async fn main() -> Result<(), AppError> {
    let meter_provider = configure_telemetry();

    let clock = Clock::new_tokio();

    // Create shared configuration.
    let context = ResilienceContext::new(&clock).use_metrics(&meter_provider);

    // Define a stack with a timeout layer.
    let stack = (
        Timeout::layer("my_timeout", &context)
            // Set the timeout duration.
            .timeout(TIMEOUT_DURATION)
            // Create the error output for a timeout.
            .timeout_error(|args| app_err!("timeout occurred, timeout: {}ms", args.timeout().as_millis())),
        Execute::new({
            let clock = clock.clone();
            move |_input| {
                let clock = clock.clone();
                async move {
                    clock.delay(PROCESSING_DELAY).await; // Add enough delay to trigger the timeout.
                    Ok(())
                }
            }
        }),
    );

    // Create the service from the stack.
    let service = stack.into_service();

    for i in 0..10 {
        // Execute the service. The call returns a timeout error.
        let timeout_error = service.execute(i.to_string()).await.unwrap_err();
        println!("{i} attempt, error: {timeout_error}");
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
