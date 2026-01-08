// Copyright (c) Microsoft Corporation.

//! Custom middleware layer.
//!
//! Shows how to define and compose custom middleware layers.

use layered::Execute;
use layered::prelude::*;

#[tokio::main]
async fn main() {
    // Stack multiple logging layers: layer-1 -> layer-2 -> core service
    let execution_stack = (
        Logging::layer("layer-1"),
        Logging::layer("layer-2"),
        Execute::new(|input| async move {
            println!("executing input: {input}");
            input
        }),
    );

    // Build and execute
    let service = execution_stack.build();
    let _output = service.execute("Hello, World!".to_string()).await;
}

/// Logging middleware that wraps a service.
#[derive(Debug)]
pub struct Logging<S> {
    inner: S,
    id: &'static str,
}

/// Layer for creating logging middleware.
#[derive(Debug)]
pub struct LoggingLayer {
    id: &'static str,
}

impl Logging<()> {
    /// Creates a logging layer with the given identifier.
    #[must_use]
    pub fn layer(id: &'static str) -> LoggingLayer {
        LoggingLayer { id }
    }
}

/// Logs before and after service execution.
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

/// Wraps services with logging.
impl<S> Layer<S> for LoggingLayer {
    type Service = Logging<S>;

    fn layer(&self, inner: S) -> Self::Service {
        Logging { inner, id: self.id }
    }
}
