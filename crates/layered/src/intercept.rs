// Copyright (c) Microsoft Corporation.

use std::fmt::Debug;
use std::ops::ControlFlow;
use std::sync::Arc;

use crate::Service;

/// Middleware for observing and modifying service inputs and outputs.
///
/// Useful for logging, debugging, metrics, validation, and other cross-cutting concerns.
///
/// # Examples
///
/// Simple usage that observes inputs and outputs without modification:
///
/// ```rust
/// # use layered::{Execute, Stack, Intercept, Service};
/// # async fn example() {
/// let execution_stack = (
///     Intercept::layer()
///         .on_input(|input| println!("request: {input}"))
///         .on_output(|output| println!("response: {output}")),
///     Execute::new(|input: String| async move { input }),
/// );
///
/// let service = execution_stack.build();
/// let response = service.execute("input".to_string()).await;
/// # }
/// ```
///
/// Advanced usage of `Intercept` allows you to modify and observe inputs and outputs:
///
/// ```rust
/// # use layered::{Execute, Stack, Intercept, Service};
/// # async fn example() {
/// let execution_stack = (
///     Intercept::<String, String, _>::layer()
///         .on_input(|input| println!("request: {input}")) // input observers are called first
///         .on_input(|input| println!("another: {input}")) // multiple observers supported
///         .debug_input() // convenience method to print inputs with `dbg!`
///         .modify_input(|input| input.to_uppercase()) // then inputs are modified
///         .modify_input(|input| input.to_lowercase()) // multiple modifications supported
///         .on_output(|output| println!("response: {output}")) // output observers called first
///         .on_output(|output| println!("another response: {output}")) // multiple observers supported
///         .debug_output() // convenience method to print outputs with `dbg!`
///         .modify_output(|output| output.trim().to_string()) // then outputs are modified
///         .modify_output(|output| format!("result: {output}")), // multiple modifications supported
///     Execute::new(|input: String| async move { input }),
/// );
///
/// let service = execution_stack.build();
/// let response = service.execute("input".to_string()).await;
/// # }
/// ```
#[derive(Clone, Debug)]
pub struct Intercept<In, Out, S> {
    inner: Arc<InterceptInner<In, Out>>,
    service: S,
}

/// Builder for creating `Intercept` middleware.
///
/// Provides a fluent API for configuring input and output observers and modifiers.
/// Create with `Intercept::layer()`.
///
/// # Examples
///
/// ```rust
/// # use layered::{Execute, Stack, Intercept, Service};
/// # async fn example() {
/// let execution_stack = (
///     Intercept::layer(), // Create a new interception layer
///     Execute::new(|input: String| async move { input }),
/// );
///
/// let service = execution_stack.build();
/// let response = service.execute("input".to_string()).await;
/// # }
/// ```
#[derive(Clone, Debug)]
pub struct InterceptLayer<In, Out> {
    on_input: Vec<OnInput<In>>,
    modify_input: Vec<ModifyInput<In, Out>>,
    modify_output: Vec<ModifyOutput<Out>>,
    on_output: Vec<OnOutput<Out>>,
}

impl<In, Out> Intercept<In, Out, ()> {
    /// Creates a new `InterceptLayer` for building interception middleware.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # use layered::{Execute, Stack, Intercept, Service};
    /// # async fn example() {
    /// let execution_stack = (
    ///     Intercept::layer(), // Create a new interception layer, no observers yet
    ///     Execute::new(|input: String| async move { input }),
    /// );
    ///
    /// let service = execution_stack.build();
    /// let response = service.execute("input".to_string()).await;
    /// # }
    /// ```
    #[must_use]
    pub fn layer() -> InterceptLayer<In, Out> {
        InterceptLayer {
            on_input: Vec::default(),
            modify_input: Vec::default(),
            modify_output: Vec::default(),
            on_output: Vec::default(),
        }
    }
}

impl<In: Send, Out, S> Service<In> for Intercept<In, Out, S>
where
    S: Service<In, Out = Out>,
{
    type Out = Out;

    /// Executes the wrapped service with interception and modification.
    ///
    /// Execution order: input observers → input modifications → service execution
    /// → output observers → output modifications. Input modifications can short-circuit
    /// execution by returning `ControlFlow::Break`.
    async fn execute(&self, mut input: In) -> Self::Out {
        match self.inner.before_execute(input) {
            ControlFlow::Break(output) => return output,
            ControlFlow::Continue(new_input) => input = new_input,
        }

        let output = self.service.execute(input).await;

        self.inner.after_execute(output)
    }
}

#[cfg(any(feature = "tower-service", test))]
impl<Req, Res, Err, S> tower_service::Service<Req> for Intercept<Req, Result<Res, Err>, S>
where
    Err: Send + 'static,
    Req: Send + 'static,
    Res: Send + 'static,
    S: tower_service::Service<Req, Response = Res, Error = Err> + Send + Sync + 'static,
    S::Future: Send + 'static,
{
    type Response = Res;
    type Error = Err;
    type Future = std::pin::Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, cx: &mut std::task::Context<'_>) -> std::task::Poll<Result<(), Self::Error>> {
        self.service.poll_ready(cx)
    }

    fn call(&mut self, req: Req) -> Self::Future {
        let result = self.inner.before_execute(req);
        let req = match result {
            ControlFlow::Break(result) => return Box::pin(async move { result }),
            ControlFlow::Continue(new_req) => new_req,
        };

        let inner = Arc::clone(&self.inner);
        let future = self.service.call(req);

        Box::pin(async move {
            let r = future.await;
            inner.after_execute(r)
        })
    }
}

impl<In, Out> InterceptLayer<In, Out> {
    /// Adds an observer for incoming inputs.
    ///
    /// Called before input modifications. Multiple observers execute in registration order.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # use layered::{Execute, Stack, Intercept, Service};
    /// # async fn example() {
    /// let execution_stack = (
    ///     Intercept::layer()
    ///         .on_input(|input| println!("processing: {input}"))
    ///         .on_input(|input| println!("another: {input}")),
    ///     Execute::new(|input: String| async move { input }),
    /// );
    ///
    /// let service = execution_stack.build();
    /// let response = service.execute("input".to_string()).await;
    /// # }
    /// ```
    #[must_use]
    pub fn on_input<F>(mut self, f: F) -> Self
    where
        F: Fn(&In) + Send + Sync + 'static,
    {
        self.on_input.push(OnInput(Arc::new(f)));
        self
    }

    /// Adds an observer for outgoing outputs.
    ///
    /// Called before output modifications. Multiple observers execute in registration order.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # use layered::{Execute, Stack, Intercept, Service};
    /// # async fn example() {
    /// let execution_stack = (
    ///     Intercept::layer()
    ///         .on_output(|output| println!("response: {output}"))
    ///         .on_output(|output| println!("another response: {output}")),
    ///     Execute::new(|input: String| async move { input }),
    /// );
    ///
    /// let service = execution_stack.build();
    /// let response = service.execute("input".to_string()).await;
    /// # }
    /// ```
    #[must_use]
    pub fn on_output<F>(mut self, f: F) -> Self
    where
        F: Fn(&Out) + Send + Sync + 'static,
    {
        self.on_output.push(OnOutput(Arc::new(f)));
        self
    }

    /// Adds a transformation for incoming inputs.
    ///
    /// Transforms inputs before service execution. Multiple modifications apply
    /// in registration order, each receiving the previous output.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # use layered::{Execute, Stack, Intercept, Service};
    /// # async fn example() {
    /// let execution_stack = (
    ///     Intercept::layer()
    ///         .modify_input(|input: String| input.trim().to_string())
    ///         .modify_input(|input| input.to_lowercase()),
    ///     Execute::new(|input: String| async move { input }),
    /// );
    ///
    /// let service = execution_stack.build();
    /// let response = service.execute("input".to_string()).await;
    /// # }
    /// ```
    #[must_use]
    pub fn modify_input<F>(self, f: F) -> Self
    where
        F: Fn(In) -> In + Send + Sync + 'static,
    {
        self.input_control_flow(move |input| ControlFlow::Continue(f(input)))
    }

    /// Adds a modification function with control flow for incoming requests.
    fn input_control_flow<F>(mut self, f: F) -> Self
    where
        F: Fn(In) -> ControlFlow<Out, In> + Send + Sync + 'static,
    {
        self.modify_input.push(ModifyInput(Arc::new(f)));
        self
    }

    /// Adds a transformation for outgoing outputs.
    ///
    /// Transforms outputs after service execution. Multiple modifications apply
    /// in registration order, each receiving the previous output.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # use layered::{Execute, Stack, Intercept, Service};
    /// # async fn example() {
    /// let execution_stack = (
    ///     Intercept::layer()
    ///         .modify_output(|output: String| output.trim().to_string())
    ///         .modify_output(|output| format!("Result: {}", output)),
    ///     Execute::new(|input: String| async move { input }),
    /// );
    ///
    /// let service = execution_stack.build();
    /// let response = service.execute("input".to_string()).await;
    /// # }
    /// ```
    #[must_use]
    pub fn modify_output<F>(mut self, f: F) -> Self
    where
        F: Fn(Out) -> Out + Send + Sync + 'static,
    {
        self.modify_output.push(ModifyOutput(Arc::new(f)));
        self
    }
}

impl<In: Debug, Out> InterceptLayer<In, Out> {
    /// Adds debug logging for incoming inputs using `dbg!`.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # use layered::{Execute, Stack, Intercept, Service};
    /// # async fn example() {
    /// let execution_stack = (
    ///     Intercept::layer().debug_input(), // print input with dbg!
    ///     Execute::new(|input: String| async move { input }),
    /// );
    ///
    /// let service = execution_stack.build();
    /// let response = service.execute("input".to_string()).await;
    /// # }
    /// ```
    #[must_use]
    pub fn debug_input(self) -> Self {
        self.on_input(|input| {
            dbg!(input);
        })
    }
}

impl<In, Out: Debug> InterceptLayer<In, Out> {
    /// Adds debug logging for outgoing outputs using `dbg!`.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # use layered::{Execute, Stack, Intercept, Service};
    /// # async fn example() {
    /// let execution_stack = (
    ///     Intercept::layer().debug_output(), // print outputs with dbg!
    ///     Execute::new(|input: String| async move { input }),
    /// );
    ///
    /// let service = execution_stack.build();
    /// let response = service.execute("input".to_string()).await;
    /// # }
    /// ```
    #[must_use]
    pub fn debug_output(self) -> Self {
        self.on_output(|output| {
            dbg!(output);
        })
    }
}

impl<In, Out, S> crate::Layer<S> for InterceptLayer<In, Out> {
    type Service = Intercept<In, Out, S>;

    fn layer(&self, inner: S) -> Self::Service {
        let intercept_inner = InterceptInner {
            modify_input: self.modify_input.clone().into(),
            on_input: self.on_input.clone().into(),
            modify_output: self.modify_output.clone().into(),
            on_output: self.on_output.clone().into(),
        };

        Intercept {
            inner: Arc::new(intercept_inner),
            service: inner,
        }
    }
}

struct OnInput<In>(Arc<dyn Fn(&In) + Send + Sync>);

impl<In> Clone for OnInput<In> {
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
}

impl<In> Debug for OnInput<In> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OnInput").finish()
    }
}

struct OnOutput<Out>(Arc<dyn Fn(&Out) + Send + Sync>);

impl<Out> Clone for OnOutput<Out> {
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
}

impl<Out> Debug for OnOutput<Out> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OnOutput").finish()
    }
}

struct ModifyInput<In, Out>(Arc<dyn Fn(In) -> ControlFlow<Out, In> + Send + Sync>);

impl<In, Out> Clone for ModifyInput<In, Out> {
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
}

impl<In, Out> Debug for ModifyInput<In, Out> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ModifyInput").finish()
    }
}

struct ModifyOutput<Out>(Arc<dyn Fn(Out) -> Out + Send + Sync>);

impl<Out> Clone for ModifyOutput<Out> {
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
}

impl<Out> Debug for ModifyOutput<Out> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ModifyOutput").finish()
    }
}

#[derive(Debug)]
struct InterceptInner<In, Out> {
    modify_input: Arc<[ModifyInput<In, Out>]>,
    on_input: Arc<[OnInput<In>]>,
    modify_output: Arc<[ModifyOutput<Out>]>,
    on_output: Arc<[OnOutput<Out>]>,
}

impl<In, Out> InterceptInner<In, Out> {
    #[inline]
    fn before_execute(&self, mut input: In) -> ControlFlow<Out, In> {
        for on_input in self.on_input.iter() {
            on_input.0(&input);
        }

        for modify in self.modify_input.iter() {
            match modify.0(input) {
                ControlFlow::Break(output) => return ControlFlow::Break(output),
                ControlFlow::Continue(new_input) => input = new_input,
            }
        }

        ControlFlow::Continue(input)
    }

    #[inline]
    fn after_execute(&self, mut output: Out) -> Out {
        for on_output in self.on_output.iter() {
            on_output.0(&output);
        }

        for modify in self.modify_output.iter() {
            output = modify.0(output);
        }

        output
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use std::future::poll_fn;
    use std::pin::Pin;
    use std::sync::atomic::{AtomicU16, Ordering};
    use std::task::{Context, Poll};

    use futures::executor::block_on;
    use tower_service::Service as TowerService;

    use super::*;
    use crate::{Execute, Layer, Stack};

    #[test]
    pub fn ensure_types() {
        static_assertions::assert_impl_all!(Intercept::<String, String, ()>: Debug, Clone, Send, Sync);
        static_assertions::assert_impl_all!(InterceptLayer::<String, String>: Debug, Clone, Send, Sync);
    }

    #[test]
    #[expect(clippy::similar_names, reason = "Test")]
    fn input_modification_order() {
        let called = Arc::new(AtomicU16::default());
        let called_clone = Arc::clone(&called);

        let called2 = Arc::new(AtomicU16::default());
        let called2_clone = Arc::clone(&called2);

        let execution_stack = (
            Intercept::layer()
                .modify_input(|input: String| format!("{input}1"))
                .modify_input(|input: String| format!("{input}2"))
                .on_input(move |_input| {
                    called.fetch_add(1, Ordering::Relaxed);
                })
                .on_input(move |_input| {
                    called2.fetch_add(1, Ordering::Relaxed);
                }),
            Execute::new(|input: String| async move { input }),
        );

        let service = execution_stack.build();
        let response = block_on(service.execute("test".to_string()));
        assert_eq!(called_clone.load(Ordering::Relaxed), 1);
        assert_eq!(called2_clone.load(Ordering::Relaxed), 1);
        assert_eq!(response, "test12");
    }

    #[test]
    #[expect(clippy::similar_names, reason = "Test")]
    fn out_modification_order() {
        let called = Arc::new(AtomicU16::default());
        let called_clone = Arc::clone(&called);

        let called2 = Arc::new(AtomicU16::default());
        let called2_clone = Arc::clone(&called2);

        let execution_stack = (
            Intercept::layer()
                .modify_output(|output: String| format!("{output}1"))
                .modify_output(|output: String| format!("{output}2"))
                .on_output(move |_output| {
                    called.fetch_add(1, Ordering::Relaxed);
                })
                .on_output(move |_output| {
                    called2.fetch_add(1, Ordering::Relaxed);
                }),
            Execute::new(|input: String| async move { input }),
        );

        let service = execution_stack.build();
        let response = block_on(service.execute("test".to_string()));
        assert_eq!(called_clone.load(Ordering::Relaxed), 1);
        assert_eq!(called2_clone.load(Ordering::Relaxed), 1);
        assert_eq!(response, "test12");
    }

    #[test]
    #[expect(clippy::similar_names, reason = "Test")]
    fn tower_service() {
        let called = Arc::new(AtomicU16::default());
        let called_clone = Arc::clone(&called);

        let called2 = Arc::new(AtomicU16::default());
        let called2_clone = Arc::clone(&called2);

        let execution_stack = (
            Intercept::layer()
                .modify_input(|input: String| format!("{input}1"))
                .modify_input(|input: String| format!("{input}2"))
                .on_input(move |_input| {
                    called.fetch_add(1, Ordering::Relaxed);
                })
                .on_input(move |_input| {
                    called2.fetch_add(1, Ordering::Relaxed);
                }),
            Execute::new(|input: String| async move { Ok::<_, String>(input) }),
        );

        let mut service = execution_stack.build();
        let future = async move {
            poll_fn(|cx| service.poll_ready(cx)).await.unwrap();
            let response = service.call("test".to_string()).await.unwrap();
            assert_eq!(response, "test12");
        };

        block_on(future);

        assert_eq!(called_clone.load(Ordering::Relaxed), 1);
        assert_eq!(called2_clone.load(Ordering::Relaxed), 1);
    }

    // Mock service for testing poll_ready behavior
    struct MockService {
        poll_ready_response: Poll<Result<(), String>>,
    }

    impl MockService {
        fn new(poll_ready_response: Poll<Result<(), String>>) -> Self {
            Self { poll_ready_response }
        }
    }

    impl TowerService<String> for MockService {
        type Response = String;
        type Error = String;
        type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

        fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
            self.poll_ready_response.clone()
        }

        fn call(&mut self, req: String) -> Self::Future {
            Box::pin(async move { Ok(req) })
        }
    }

    #[test]
    fn poll_ready_propagates_pending() {
        let mock_service = MockService::new(Poll::Pending);
        let intercept_layer = InterceptLayer {
            on_input: Vec::default(),
            modify_input: Vec::default(),
            modify_output: Vec::default(),
            on_output: Vec::default(),
        };
        let mut intercept = intercept_layer.layer(mock_service);

        let waker = futures::task::noop_waker();
        let mut cx = Context::from_waker(&waker);

        let result = intercept.poll_ready(&mut cx);
        assert!(result.is_pending());
    }

    #[test]
    fn poll_ready_propagates_error() {
        let mock_service = MockService::new(Poll::Ready(Err("service error".to_string())));
        let intercept_layer = InterceptLayer {
            on_input: Vec::default(),
            modify_input: Vec::default(),
            modify_output: Vec::default(),
            on_output: Vec::default(),
        };
        let mut intercept = intercept_layer.layer(mock_service);

        let waker = futures::task::noop_waker();
        let mut cx = Context::from_waker(&waker);

        let result = intercept.poll_ready(&mut cx);
        match result {
            Poll::Ready(Err(err)) => assert_eq!(err, "service error"),
            _ => panic!("Expected Poll::Ready(Err), got {result:?}"),
        }
    }

    #[test]
    fn poll_ready_propagates_success() {
        let mock_service = MockService::new(Poll::Ready(Ok(())));
        let intercept_layer = InterceptLayer {
            on_input: Vec::default(),
            modify_input: Vec::default(),
            modify_output: Vec::default(),
            on_output: Vec::default(),
        };
        let mut intercept = intercept_layer.layer(mock_service);

        let waker = futures::task::noop_waker();
        let mut cx = Context::from_waker(&waker);

        let result = intercept.poll_ready(&mut cx);
        match result {
            Poll::Ready(Ok(())) => (),
            _ => panic!("Expected Poll::Ready(Ok(())), got {result:?}"),
        }
    }
}
