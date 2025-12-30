// Copyright (c) Microsoft Corporation.

//! Custom Layer Example
//!
//! Demonstrates how to define a custom middleware layer that adds logging functionality and
//! how to compose it with other layers in an execution stack.

use layered::Execute;
use layered::prelude::*;

#[tokio::main]
async fn main() {
    // Create an execution stack with multiple logging layers.
    // Input flow: layer-1 -> layer-2 -> core service
    let execution_stack = (
        Logging::layer("layer-1"),
        Logging::layer("layer-2"),
        Execute::new(|input| async move {
            println!("executing input: {input}");
            input
        }),
    );

    // Build a service and execute an input.
    let service = execution_stack.build();
    let _output = service.execute("Hello, World!".to_string()).await;
}

/// A logging middleware that wraps an inner service.
#[derive(Debug)]
pub struct Logging<S> {
    inner: S,
    id: &'static str,
}

/// A layer for creating logging middleware.
#[derive(Debug)]
pub struct LoggingLayer {
    id: &'static str,
}

impl Logging<()> {
    /// Creates a new logging layer with the specified identifier.
    #[must_use]
    pub fn layer(id: &'static str) -> LoggingLayer {
        LoggingLayer { id }
    }
}

/// Service implementation that logs before and after execution.
impl<S, In: Send, Out> Service<In> for Logging<S>
where
    S: Service<In, Out = Out>,
{
    type Out = Out;

    async fn execute(&self, input: In) -> Self::Out {
        println!("{}: executing input...", self.id);
        let result = self.inner.execute(input).await;
        println!("{}: executing input...done", self.id);

        result
    }
}

/// Layer implementation for service composition.
impl<S> Layer<S> for LoggingLayer {
    type Service = Logging<S>;

    fn layer(&self, inner: S) -> Self::Service {
        Logging { inner, id: self.id }
    }
}
