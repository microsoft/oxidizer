// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Basic service with middleware.
//!
//! Shows a simple service with intercept middleware for logging.

use layered::prelude::*;
use layered::{Execute, Intercept};

#[tokio::main]
async fn main() {
    let stack = (
        Intercept::layer()
            .on_input(|i| println!("on input: {i}"))
            .on_output(|o| println!("on output: {o}")),
        Execute::new(|input: String| async move {
            println!("executing input: {input}");
            input.to_uppercase()
        }),
    );

    // Build the service
    let service = stack.into_service();

    // Execute an input
    let _output = service.execute("Hello, World!".to_string()).await;
}
