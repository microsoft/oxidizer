// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! This example retries calls with cloned input and simple recovery rules.

use std::io::Error;

use layered::{Execute, Service, Stack};
use ohno::AppError;
use seatbelt::retry::Retry;
use seatbelt::{RecoveryInfo, ResilienceContext};
use tick::Clock;

#[tokio::main]
async fn main() -> Result<(), AppError> {
    let clock = Clock::new_tokio();
    let context = ResilienceContext::new(&clock);

    // Define a stack with a retry layer.
    let stack = (
        Retry::layer("my_retry", &context)
            .clone_input() // Automatically clone input for retries
            .recovery_with(|output, _args| match output {
                Ok(_) => RecoveryInfo::never(),
                Err(_) => RecoveryInfo::retry(),
            }),
        Execute::new(execute_operation),
    );

    // Create the service from the stack.
    let service = stack.into_service();

    match service.execute("value".to_string()).await {
        Ok(output) => println!("execution succeeded, result: {output}"),
        Err(e) => println!("execution failed, error: {e}"),
    }

    Ok(())
}

// Return a temporary error for 10% of calls.
async fn execute_operation(input: String) -> Result<String, Error> {
    if fastrand::i16(0..10) > 8 {
        Err(Error::other("transient execution error"))
    } else {
        Ok(input)
    }
}
