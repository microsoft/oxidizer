// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

/// An async function `In → Out` that processes inputs.
///
/// This trait is the foundation for building composable services.
/// Implement it directly for custom services, or use [`Execute`][crate::Execute]
/// to wrap closures.
///
/// See the [crate documentation][crate] for usage examples and layer composition.
#[cfg_attr(any(test, feature = "dynamic-service"), dynosaur::dynosaur(pub DynService = dyn(box) Service, bridge(none)))]
pub trait Service<In>: Send + Sync {
    /// The output type returned by this service.
    type Out;

    /// Processes the input and returns the output.
    ///
    /// The returned future must be [`Send`] for compatibility with multi-threaded
    /// async runtimes.
    ///
    /// # Examples
    ///
    /// ```
    /// use layered::Service;
    ///
    /// struct EchoService;
    ///
    /// impl Service<String> for EchoService {
    ///     type Out = String;
    ///
    ///     async fn execute(&self, input: String) -> Self::Out {
    ///         input
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

#[cfg_attr(coverage_nightly, coverage(off))]
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
