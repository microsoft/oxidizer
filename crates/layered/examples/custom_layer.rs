// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Custom middleware layer.
//!
//! This example defines and composes custom middleware layers.

use layered::Execute;
use layered::prelude::*;

#[tokio::main]
async fn main() {
    // Apply `layer-1`, then `layer-2`, before the root service.
    let stack = (
        Logging::layer("layer-1"),
        Logging::layer("layer-2"),
        Execute::new(|input| async move {
            println!("executing input: {input}");
            input
        }),
    );

    // Build and execute the service.
    let service = stack.into_service();
    let _output = service.execute("Hello, World!".to_string()).await;
}

/// Middleware that logs service execution.
#[derive(Debug)]
pub struct Logging<S> {
    inner: S,
    id: &'static str,
}

/// Creates logging middleware.
#[derive(Debug)]
pub struct LoggingLayer {
    id: &'static str,
}

impl Logging<()> {
    /// Creates a logging layer with an identifier.
    #[must_use]
    pub fn layer(id: &'static str) -> LoggingLayer {
        LoggingLayer { id }
    }
}

/// Logs before and after the inner service runs.
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

/// Wraps a service with logging.
impl<S> Layer<S> for LoggingLayer {
    type Service = Logging<S>;

    fn layer(&self, inner: S) -> Self::Service {
        Logging { inner, id: self.id }
    }
}
