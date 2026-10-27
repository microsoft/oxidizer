// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Tower support for layered services.
//!
//! The [`Adapter`] type converts between Tower services and layered services.
//!
//! # Requirements
//!
//! - Each wrapped service must implement `Clone`.
//! - A layered service used through Tower must return `Result<T, E>`.
//! - Its request, response, and error types must implement `Send + 'static`.
//! - A Tower service used through layered must implement `Send + Sync`.
//! - Its request and future types must implement `Send`.
//! - The adapter handles Tower back-pressure through `poll_ready`.
//!
//! # Examples
//!
//! Use a layered service with Tower middleware:
//!
//! ```
//! use std::time::Duration;
//!
//! use layered::tower::Adapter;
//! use layered::{Execute, Service};
//! use tower::ServiceBuilder;
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let service = Execute::new(|req: String| async move {
//!     Ok::<String, std::io::Error>(format!("Processed: {}", req))
//! });
//!
//! let tower_service = ServiceBuilder::new().service(Adapter(service));
//! # Ok(())
//! # }
//! ```
//!
//! Use Tower layers in a layered stack:
//!
//! ```
//! use layered::tower::tower_layer;
//! use layered::{Execute, Service, Stack};
//! use tower_layer::Identity;
//!
//! async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let service = (
//!     tower_layer(Identity::default()),
//!     Execute::new(|req: String| async move {
//!         Ok::<String, std::io::Error>(format!("Processed: {}", req))
//!     }),
//! ).into_service();
//!
//! let result = service.execute("request".to_string()).await;
//! # Ok(())
//! # }
//! ```

use std::future::poll_fn;
use std::pin::Pin;
use std::task::{Context, Poll};

use tower_layer::Layer;

use crate::Service;

/// Converts between layered and Tower service traits.
///
/// This type wraps a service and implements the service trait from the other system.
/// It handles Tower back-pressure and the different async interfaces.
#[derive(Debug, Clone)]
pub struct Adapter<S>(pub S);

impl<S, Req, T, E> tower_service::Service<Req> for Adapter<S>
where
    S: Service<Req, Out = Result<T, E>> + Clone + 'static,
    T: Send + 'static,
    E: Send + 'static,
    Req: Send + 'static,
{
    type Response = T;
    type Error = E;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    #[cfg_attr(test, mutants::skip)] // causes unkillable mutants
    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        // Layered services do not use back-pressure, so they are always ready.
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, req: Req) -> Self::Future {
        let service = self.0.clone();
        Box::pin(async move { service.execute(req).await })
    }
}

impl<S, In: Send> Service<In> for Adapter<S>
where
    S: tower_service::Service<In> + Send + Sync + Clone,
    S::Future: Send,
{
    type Out = Result<S::Response, S::Error>;

    fn execute(&self, input: In) -> impl Future<Output = Self::Out> + Send {
        let mut clone = self.0.clone();

        async move {
            // Wait until the Tower service is ready.
            poll_fn(|cx| clone.poll_ready(cx)).await?;
            // Send the request to the Tower service.
            clone.call(input).await
        }
    }
}

/// Wraps a Tower layer for use in a layered stack.
///
/// # Examples
///
/// ```
/// use layered::tower::tower_layer;
/// use tower::layer::util::Identity;
///
/// let identity_layer = tower_layer(Identity::new());
/// ```
pub fn tower_layer<L>(tower_layer: L) -> AdapterLayer<L> {
    AdapterLayer(tower_layer)
}

/// Applies a Tower layer to a layered service.
///
/// The adapter converts the service traits before and after the Tower layer runs.
#[derive(Debug, Clone)]
pub struct AdapterLayer<L>(L);

impl<L, S> Layer<S> for AdapterLayer<L>
where
    L: Layer<Adapter<S>> + Clone,
{
    type Service = Adapter<L::Service>;

    fn layer(&self, inner: S) -> Self::Service {
        // Convert the layered service to a Tower service.
        let tower_adapted = Adapter(inner);
        // Apply the Tower layer.
        let tower_layered = self.0.layer(tower_adapted);
        // Convert the result back to a layered service.
        Adapter(tower_layered)
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use futures::executor::block_on;
    use tower::service_fn;
    use tower_service::Service as TowerService;

    use super::*;
    use crate::testing::MockService;

    #[test]
    fn adapt_tower_ok() {
        let service = service_fn(|req: u32| async move { Ok::<_, ()>(req + 1) });
        let service = Adapter(service);

        let result = block_on(service.execute(0));

        assert_eq!(result, Ok(1));
    }

    #[test]
    fn adapt_tower_ensure_poll_error_respected() {
        let service = MockService::new(Poll::Ready(Err("error".to_string())), Err("call error".to_string()));
        let service = Adapter(service);

        let result = block_on(service.execute("request".to_string()));

        assert_eq!(result, Err("error".to_string()));
    }

    #[test]
    fn adapt_oxidizer_ok() {
        let mock_service = MockService::new(Poll::Ready(Ok(())), Ok("success".to_string()));
        let mut service = Adapter(mock_service);

        let result = block_on(async move { service.call("request".to_string()).await });

        assert_eq!(result, Ok("success".to_string()));
    }

    #[test]
    fn poll_ready_always_returns_ready_ok() {
        let mock_service = MockService::new(Poll::Ready(Ok(())), Ok("success".to_string()));
        let mut adapter = Adapter(mock_service);

        let waker = futures::task::noop_waker();
        let mut cx = Context::from_waker(&waker);

        let result = adapter.poll_ready(&mut cx);
        assert_eq!(result, Poll::Ready(Ok(())));
    }

    #[test]
    fn poll_ready_consistent_behavior() {
        let mock_service = MockService::new(Poll::Ready(Ok(())), Ok("success".to_string()));
        let mut adapter = Adapter(mock_service);

        let waker = futures::task::noop_waker();
        let mut cx = Context::from_waker(&waker);

        // Every call returns the same ready result.
        for _ in 0..3 {
            let result = adapter.poll_ready(&mut cx);
            assert_eq!(result, Poll::Ready(Ok(())));
        }
    }

    #[test]
    fn poll_ready_with_mock_service() {
        let mock_service = MockService::new(Poll::Ready(Ok(())), Ok("success".to_string()));
        let mut mock_adapter = Adapter(mock_service);

        let waker = futures::task::noop_waker();
        let mut cx = Context::from_waker(&waker);

        assert_eq!(mock_adapter.poll_ready(&mut cx), Poll::Ready(Ok(())));
    }

    #[test]
    fn poll_ready_mutation_equivalence() {
        // Both constructors produce the same ready result.
        let mock_service = MockService::new(Poll::Ready(Ok(())), Ok("success".to_string()));
        let mut adapter = Adapter(mock_service);

        let waker = futures::task::noop_waker();
        let mut cx = Context::from_waker(&waker);

        let result1 = adapter.poll_ready(&mut cx);
        let result2 = Poll::from(Ok::<(), String>(()));

        // Compare both values with `Poll::Ready(Ok(()))`.
        assert_eq!(result1, Poll::Ready(Ok(())));
        assert_eq!(result2, Poll::Ready(Ok(())));
        assert_eq!(result1, result2);
    }

    #[test]
    fn adapter_execute_fails_when_tower_service_poll_ready_errors() {
        let mock_service = MockService::new(Poll::Ready(Err("service unavailable".to_string())), Ok("success".to_string()));
        let service = Adapter(mock_service);

        let result = block_on(service.execute("request".to_string()));

        assert_eq!(result, Err("service unavailable".to_string()));
    }

    #[test]
    fn tower_layer_adapter() {
        use tower_layer::Identity;

        use crate::{Execute, Stack};

        let stack = (tower_layer(Identity::new()), Execute::new(|x: i32| async move { Ok::<_, ()>(x) }));
        let svc = stack.into_service();
        assert_eq!(block_on(svc.execute(42)), Ok(42));
    }
}
