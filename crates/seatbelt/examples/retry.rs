// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Basic retry middleware example with automatic input cloning and simple recovery logic.

use std::io::Error;

use layered::{Execute, Service, Stack};
use ohno::AppError;
use seatbelt::retry::Retry;
use seatbelt::{Context, RecoveryInfo};
use tick::Clock;

#[tokio::main]
async fn main() -> Result<(), AppError> {
    let clock = Clock::new_tokio();
    let context = Context::new(&clock);

    // Define stack with retry layer
    let stack = (
        Retry::layer("my_retry", &context)
            .clone_input() // Automatically clone input for retries
            .recovery_with(|output, _args| match output {
                Ok(_) => RecoveryInfo::never(),
                Err(_) => RecoveryInfo::retry(),
            }),
        Execute::new(execute_operation),
    );

    // Create the service from the stack
    let service = stack.build();

    match service.execute("value".to_string()).await {
        Ok(output) => println!("execution succeeded, result: {output}"),
        Err(e) => println!("execution failed, error: {e}"),
    }

    Ok(())
}

// 20% chance of failing with a transient error
async fn execute_operation(input: String) -> Result<String, Error> {
    if fastrand::i16(0..10) > 8 {
        Err(Error::other("transient execution error"))
    } else {
        Ok(input)
    }
}
