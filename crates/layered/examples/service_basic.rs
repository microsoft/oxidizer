// Copyright (c) Microsoft Corporation.

//! Basic Example
//!
//! Demonstrates how we can build a simple service consisting of a single middleware
//! and a root service.

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
    let service = stack.build();

    // Execute an input
    let _output = service.execute("Hello, World!".to_string()).await;
}
