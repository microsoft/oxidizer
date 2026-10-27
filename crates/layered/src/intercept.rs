// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt::Debug;
use std::ops::ControlFlow;
#[cfg(any(feature = "tower-service", test))]
use std::pin::Pin;
use std::sync::Arc;
#[cfg(any(feature = "tower-service", test))]
use std::task::{Context, Poll};

use crate::Service;

/// Observes or changes service inputs and outputs.
///
/// Use this middleware for logging, debugging, metrics, and validation.
///
/// # Examples
///
/// Observe inputs and outputs without changing them:
///
/// ```
/// # use layered::{Execute, Stack, Intercept, Service};
/// # async fn example() {
/// let stack = (
///     Intercept::layer()
///         .on_input(|input| println!("request: {input}"))
///         .on_output(|output| println!("response: {output}")),
///     Execute::new(|input: String| async move { input }),
/// );
///
/// let service = stack.into_service();
/// let response = service.execute("input".to_string()).await;
/// # }
/// ```
///
/// Observe and change inputs and outputs:
///
/// ```
/// # use layered::{Execute, Stack, Intercept, Service};
/// # async fn example() {
/// let stack = (
///     Intercept::<String, String, _>::layer()
///         .on_input(|input| println!("request: {input}")) // Observe inputs first.
///         .on_input(|input| println!("another: {input}")) // Add another input observer.
///         .modify_input(|input| input.to_uppercase()) // Then change the input.
///         .modify_input(|input| input.to_lowercase()) // Apply changes in registration order.
///         .on_output(|output| println!("response: {output}")) // Observe outputs first.
///         .on_output(|output| println!("another response: {output}")) // Add another output observer.
///         .modify_output(|output| output.trim().to_string()) // Then change the output.
///         .modify_output(|output| format!("result: {output}")), // Apply changes in registration order.
///     Execute::new(|input: String| async move { input }),
/// );
///
/// let service = stack.into_service();
/// let response = service.execute("input".to_string()).await;
/// # }
/// ```
pub struct Intercept<In, Out, S> {
    inner: Arc<InterceptInner<In, Out>>,
    service: S,
}

impl<In, Out, S: Clone> Clone for Intercept<In, Out, S> {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
            service: self.service.clone(),
        }
    }
}

impl<In, Out, S: Debug> Debug for Intercept<In, Out, S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Intercept").field("service", &self.service).finish_non_exhaustive()
    }
}

/// Builds `Intercept` middleware.
///
/// Call `Intercept::layer()` first. Then add input and output observers or modifiers.
///
/// # Examples
///
/// ```
/// # use layered::{Execute, Stack, Intercept, Service};
/// # async fn example() {
/// let stack = (
///     Intercept::layer(), // Create an interception layer.
///     Execute::new(|input: String| async move { input }),
/// );
///
/// let service = stack.into_service();
/// let response = service.execute("input".to_string()).await;
/// # }
/// ```
#[derive(Clone)]
pub struct InterceptLayer<In, Out> {
    on_input: Vec<OnInput<In>>,
    modify_input: Vec<ModifyInput<In, Out>>,
    modify_output: Vec<ModifyOutput<Out>>,
    on_output: Vec<OnOutput<Out>>,
}

impl<In, Out> Debug for InterceptLayer<In, Out> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InterceptLayer")
            .field("on_input", &self.on_input.len())
            .field("modify_input", &self.modify_input.len())
            .field("modify_output", &self.modify_output.len())
            .field("on_output", &self.on_output.len())
            .finish_non_exhaustive()
    }
}

impl<In, Out> Intercept<In, Out, ()> {
    /// Creates an `InterceptLayer`.
    ///
    /// # Examples
    ///
    /// ```
    /// # use layered::{Execute, Stack, Intercept, Service};
    /// # async fn example() {
    /// let stack = (
    ///     Intercept::layer(), // Create a layer without observers.
    ///     Execute::new(|input: String| async move { input }),
    /// );
    ///
    /// let service = stack.into_service();
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

    /// Executes the wrapped service and applies the configured operations.
    ///
    /// The order is input observers, input modifiers, service execution, output observers,
    /// and output modifiers.
    async fn execute(&self, mut input: In) -> Self::Out {
        match self.inner.before_execute(input) {
            ControlFlow::Break(output) => return output,
            ControlFlow::Continue(new_input) => input = new_input,
        }

        let output = self.service.execute(input).await;

        self.inner.after_execute(output)
    }
}

/// The future from [`Intercept`] when Tower uses it as a
/// [`Service`](tower_service::Service).
#[cfg(any(feature = "tower-service", test))]
pub struct InterceptFuture<Out> {
    inner: Pin<Box<dyn Future<Output = Out> + Send>>,
}

#[cfg(any(feature = "tower-service", test))]
impl<Out> Debug for InterceptFuture<Out> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InterceptFuture").finish_non_exhaustive()
    }
}

#[cfg(any(feature = "tower-service", test))]
impl<Out> Future for InterceptFuture<Out> {
    type Output = Out;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.inner.as_mut().poll(cx)
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
    type Future = InterceptFuture<Result<Res, Err>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.service.poll_ready(cx)
    }

    fn call(&mut self, req: Req) -> Self::Future {
        let result = self.inner.before_execute(req);
        let req = match result {
            ControlFlow::Break(result) => {
                return InterceptFuture {
                    inner: Box::pin(async move { result }),
                };
            }
            ControlFlow::Continue(new_req) => new_req,
        };

        let inner = Arc::clone(&self.inner);
        let future = self.service.call(req);

        InterceptFuture {
            inner: Box::pin(async move {
                let r = future.await;
                inner.after_execute(r)
            }),
        }
    }
}

impl<In, Out> InterceptLayer<In, Out> {
    /// Adds an input observer.
    ///
    /// Input observers run before input modifiers and in registration order.
    ///
    /// # Examples
    ///
    /// ```
    /// # use layered::{Execute, Stack, Intercept, Service};
    /// # async fn example() {
    /// let stack = (
    ///     Intercept::layer()
    ///         .on_input(|input| println!("processing: {input}"))
    ///         .on_input(|input| println!("another: {input}")),
    ///     Execute::new(|input: String| async move { input }),
    /// );
    ///
    /// let service = stack.into_service();
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

    /// Adds an output observer.
    ///
    /// Output observers run before output modifiers and in registration order.
    ///
    /// # Examples
    ///
    /// ```
    /// # use layered::{Execute, Stack, Intercept, Service};
    /// # async fn example() {
    /// let stack = (
    ///     Intercept::layer()
    ///         .on_output(|output| println!("response: {output}"))
    ///         .on_output(|output| println!("another response: {output}")),
    ///     Execute::new(|input: String| async move { input }),
    /// );
    ///
    /// let service = stack.into_service();
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

    /// Adds an input modifier.
    ///
    /// Input modifiers run before the service and in registration order. Each modifier
    /// receives the value from the previous modifier.
    ///
    /// # Examples
    ///
    /// ```
    /// # use layered::{Execute, Stack, Intercept, Service};
    /// # async fn example() {
    /// let stack = (
    ///     Intercept::layer()
    ///         .modify_input(|input: String| input.trim().to_string())
    ///         .modify_input(|input| input.to_lowercase()),
    ///     Execute::new(|input: String| async move { input }),
    /// );
    ///
    /// let service = stack.into_service();
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

    /// Adds an input modifier that can control execution.
    ///
    /// Return `ControlFlow::Break` to skip the service and return an output.
    pub(crate) fn input_control_flow<F>(mut self, f: F) -> Self
    where
        F: Fn(In) -> ControlFlow<Out, In> + Send + Sync + 'static,
    {
        self.modify_input.push(ModifyInput(Arc::new(f)));
        self
    }

    /// Adds an output modifier.
    ///
    /// Output modifiers run after the service and in registration order. Each modifier
    /// receives the value from the previous modifier.
    ///
    /// # Examples
    ///
    /// ```
    /// # use layered::{Execute, Stack, Intercept, Service};
    /// # async fn example() {
    /// let stack = (
    ///     Intercept::layer()
    ///         .modify_output(|output: String| output.trim().to_string())
    ///         .modify_output(|output| format!("Result: {}", output)),
    ///     Execute::new(|input: String| async move { input }),
    /// );
    ///
    /// let service = stack.into_service();
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

struct OnOutput<Out>(Arc<dyn Fn(&Out) + Send + Sync>);

impl<Out> Clone for OnOutput<Out> {
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
}

struct ModifyInput<In, Out>(Arc<dyn Fn(In) -> ControlFlow<Out, In> + Send + Sync>);

impl<In, Out> Clone for ModifyInput<In, Out> {
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
}

struct ModifyOutput<Out>(Arc<dyn Fn(Out) -> Out + Send + Sync>);

impl<Out> Clone for ModifyOutput<Out> {
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
}

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
    use std::sync::atomic::{AtomicU16, Ordering};

    use futures::executor::block_on;
    use tower_service::Service as TowerService;

    use super::*;
    use crate::{Execute, Layer, Stack};

    #[test]
    pub(crate) fn ensure_types() {
        static_assertions::assert_impl_all!(Intercept::<String, String, ()>: Debug, Clone, Send, Sync);
        static_assertions::assert_impl_all!(InterceptLayer::<String, String>: Debug, Clone, Send, Sync);
    }

    #[test]
    #[expect(clippy::similar_names, reason = "paired counters record observer calls")]
    fn input_modification_order() {
        let called = Arc::new(AtomicU16::default());
        let called_clone = Arc::clone(&called);

        let called2 = Arc::new(AtomicU16::default());
        let called2_clone = Arc::clone(&called2);

        let stack = (
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

        let service = stack.into_service();
        let response = block_on(service.execute("test".to_string()));
        assert_eq!(called_clone.load(Ordering::Relaxed), 1);
        assert_eq!(called2_clone.load(Ordering::Relaxed), 1);
        assert_eq!(response, "test12");
    }

    #[test]
    #[expect(clippy::similar_names, reason = "paired counters record observer calls")]
    fn out_modification_order() {
        let called = Arc::new(AtomicU16::default());
        let called_clone = Arc::clone(&called);

        let called2 = Arc::new(AtomicU16::default());
        let called2_clone = Arc::clone(&called2);

        let stack = (
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

        let service = stack.into_service();
        let response = block_on(service.execute("test".to_string()));
        assert_eq!(called_clone.load(Ordering::Relaxed), 1);
        assert_eq!(called2_clone.load(Ordering::Relaxed), 1);
        assert_eq!(response, "test12");
    }

    #[test]
    #[expect(clippy::similar_names, reason = "paired counters record observer calls")]
    fn tower_service() {
        let called = Arc::new(AtomicU16::default());
        let called_clone = Arc::clone(&called);

        let called2 = Arc::new(AtomicU16::default());
        let called2_clone = Arc::clone(&called2);

        let stack = (
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

        let mut service = stack.into_service();
        let future = async move {
            poll_fn(|cx| service.poll_ready(cx)).await.unwrap();
            let response = service.call("test".to_string()).await.unwrap();
            assert_eq!(response, "test12");
        };

        block_on(future);

        assert_eq!(called_clone.load(Ordering::Relaxed), 1);
        assert_eq!(called2_clone.load(Ordering::Relaxed), 1);
    }

    // This service returns a configured `poll_ready` result.
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
            _ => panic!("expected Poll::Ready(Err), got {result:?}"),
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
            _ => panic!("expected Poll::Ready(Ok(())), got {result:?}"),
        }
    }

    #[test]
    fn debug_intercept() {
        let debug_str = format!("{:?}", Intercept::<String, String, ()>::layer().layer("inner"));

        assert_eq!(debug_str, "Intercept { service: \"inner\", .. }");
    }

    #[test]
    fn debug_intercept_layer() {
        let debug_str = format!("{:?}", Intercept::<String, String, ()>::layer());

        assert_eq!(
            debug_str,
            "InterceptLayer { on_input: 0, modify_input: 0, modify_output: 0, on_output: 0, .. }"
        );
    }

    #[test]
    fn clone_intercept() {
        let cloned = Intercept::<String, String, ()>::layer().layer("inner").clone();

        assert_eq!(cloned.service, "inner");
    }

    #[test]
    fn debug_intercept_future() {
        let future: InterceptFuture<String> = InterceptFuture {
            inner: Box::pin(async { "test".to_string() }),
        };
        let debug_str = format!("{future:?}");
        assert!(debug_str.contains("InterceptFuture"));
    }

    #[test]
    fn short_circuit_layered() {
        let stack = (
            Intercept::layer().input_control_flow(|_: String| ControlFlow::Break("rejected".into())),
            Execute::new(|_: String| async { "should not run".to_string() }),
        );
        let svc = stack.into_service();
        assert_eq!(block_on(svc.execute("test".into())), "rejected");
    }

    #[test]
    fn short_circuit_tower() {
        let stack = (
            Intercept::layer().input_control_flow(|_: String| ControlFlow::Break(Ok("rejected".into()))),
            Execute::new(|_: String| async { Ok::<_, ()>("should not run".into()) }),
        );
        let mut svc = stack.into_service();
        let res = block_on(async {
            poll_fn(|cx| svc.poll_ready(cx)).await.unwrap();
            svc.call("test".into()).await
        });
        assert_eq!(res, Ok("rejected".to_string()));
    }
}
