// Copyright (c) Microsoft Corporation.

use std::fmt::{Debug, Formatter};

use crate::Service;

/// A service implementation that wraps a function for ad-hoc service creation.
///
/// `Execute` allows you to quickly create a [`Service`] from any function or closure
/// that takes an input and returns the future. This is particularly useful for:
///
/// - Converting lambdas into services without defining custom types
/// - Prototyping and testing service implementations
/// - Creating simple input handlers inline
/// - Wrapping existing async functions as services
///
/// # Examples
///
/// Creating a service from a simple async function:
///
/// ```rust
/// # use layered::{Execute, Service};
/// async fn handle_input(data: String) -> String {
///     format!("Processed: {}", data)
/// }
///
/// # async fn example() {
/// let service = Execute::new(handle_input);
/// let result = service.execute("test".to_string()).await;
/// assert_eq!(result, "Processed: test");
/// # }
/// ```
///
/// Creating a service from a closure:
///
/// ```rust
/// # use layered::{Execute, Service};
/// # async fn example() {
/// let service = Execute::new(move |x: i32| async move { x * 2 });
/// let result = service.execute(5).await;
/// assert_eq!(result, 10);
/// # }
/// ```
#[derive(Clone)]
pub struct Execute<E>(E);

impl<E> Execute<E> {
    /// Creates a new `Execute` service from a function or closure.
    ///
    /// The provided function `e` must:
    /// - Take an input of type `In`
    /// - Return a future that produces an output of type `Out`
    /// - Be `Send + Sync + 'static` for thread safety
    ///
    /// # Examples
    ///
    /// ```rust
    /// # use layered::Execute;
    /// let service = Execute::new(|msg: String| async move { format!("Echo: {}", msg) });
    /// ```
    #[must_use]
    pub fn new<In, Out, F>(e: E) -> Self
    where
        E: Fn(In) -> F + Send + Sync + 'static,
        In: Send + 'static,
        F: Future<Output = Out> + Send + 'static,
        Out: Send + 'static,
    {
        Self(e)
    }
}

impl<E, F, In, Out> Service<In> for Execute<E>
where
    E: Fn(In) -> F + Send + Sync,
    F: Future<Output = Out> + Send,
{
    type Out = Out;

    fn execute(&self, input: In) -> impl Future<Output = Self::Out> + Send {
        self.0(input)
    }
}

#[cfg(any(feature = "tower-service", test))]
impl<E, F, Req, Res, Err> tower_service::Service<Req> for Execute<E>
where
    E: Fn(Req) -> F + Send + Sync,
    F: Future<Output = Result<Res, Err>> + Send,
{
    type Response = Res;
    type Error = Err;
    type Future = F;

    fn poll_ready(&mut self, _cx: &mut std::task::Context<'_>) -> std::task::Poll<Result<(), Self::Error>> {
        std::task::Poll::Ready(Ok(()))
    }

    fn call(&mut self, req: Req) -> Self::Future {
        self.0(req)
    }
}

impl<E> Debug for Execute<E> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Execute").finish_non_exhaustive()
    }
}
