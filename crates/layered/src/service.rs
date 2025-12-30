// Copyright (c) Microsoft Corporation.

/// Core trait for building composable, asynchronous services.
///
/// The [`Service`] trait represents an asynchronous operation that transforms an input into an output.
/// It forms the foundation of service architecture, enabling you to build modular,
/// composable systems with cross-cutting concerns like timeouts, logging, retries, and rate limiting.
///
/// See the [crate level documentation][crate] for more details on how to implement custom services and layers.
#[cfg_attr(any(test, feature = "dynamic-service"), dynosaur::dynosaur(pub DynService = dyn(box) Service, bridge(none)))]
pub trait Service<In>: Send + Sync {
    /// The output type returned by this service.
    type Out;

    /// Executes the service with the given `input`.
    ///
    /// This is the core method where your service logic lives. It should:
    ///
    /// - Process the incoming input
    /// - Perform any necessary work (database queries, network calls, etc.)
    /// - Return the appropriate output
    ///
    /// Returns a future that resolves to the output. The future must be [`Send`]
    /// to ensure it can be moved between threads during async execution.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use layered::Service;
    ///
    /// struct EchoService;
    ///
    /// impl Service<String> for EchoService {
    ///     type Out = String;
    ///
    ///     async fn execute(&self, input: String) -> Self::Out {
    ///         input // Echo the input back
    ///     }
    /// }
    /// ```
    fn execute(&self, input: In) -> impl Future<Output = Self::Out> + Send;
}

impl<S, In> Service<In> for Box<S>
where
    S: Service<In>,
{
    type Out = S::Out;

    fn execute(&self, input: In) -> impl Future<Output = Self::Out> + Send {
        (**self).execute(input)
    }
}

impl<S, In> Service<In> for std::sync::Arc<S>
where
    S: Service<In>,
{
    type Out = S::Out;

    fn execute(&self, input: In) -> impl Future<Output = Self::Out> + Send {
        (**self).execute(input)
    }
}

#[cfg(test)]
mod tests {
    use futures::executor::block_on;

    use super::*;

    // A simple service that echoes the input.
    struct EchoService;

    impl Service<String> for EchoService {
        type Out = String;

        async fn execute(&self, input: String) -> Self::Out {
            input
        }
    }

    #[test]
    fn test_echo_service() {
        let service = EchoService;
        let output = block_on(service.execute("Hello, World!".to_string()));
        assert_eq!(output, "Hello, World!");
    }

    #[test]
    fn test_boxed_service() {
        let service: Box<EchoService> = Box::new(EchoService);
        let output = block_on(service.execute("Hello, Boxed World!".to_string()));
        assert_eq!(output, "Hello, Boxed World!");
    }

    #[test]
    fn test_arc_service() {
        let service: std::sync::Arc<EchoService> = std::sync::Arc::new(EchoService);
        let output = block_on(service.execute("Hello, Arc World!".to_string()));
        assert_eq!(output, "Hello, Arc World!");
    }
}
