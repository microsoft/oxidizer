// Copyright (c) Microsoft Corporation.

//! Simple timeout resilience middleware example.
//!
//! This example demonstrates the basic usage of the timeout middleware to cancel
//! long-running operations.

use std::time::Duration;

use anyhow::anyhow;
use layered::{Execute, Service, Stack};
use oxidizer_rt::Builtins;
use seatbelt::SeatbeltOptions;
use seatbelt::timeout::Timeout;

const TIMEOUT_DURATION: Duration = Duration::from_millis(100);
const PROCESSING_DELAY: Duration = Duration::from_millis(500);

#[oxidizer_rt::main]
async fn main(state: Builtins) -> anyhow::Result<()> {
    // Create common options
    let options = SeatbeltOptions::new(&state);

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
        Execute::new(move |_input| {
            let clock = state.clock().clone();
            async move {
                clock.delay(PROCESSING_DELAY).await; // Simulate some processing delay so the timeout can trigger
                Ok(())
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

    Ok(())
}
