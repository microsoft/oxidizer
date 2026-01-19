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
    // Integration tests have been moved to tests/service.rs
    // No internal tests needed for this module
}
