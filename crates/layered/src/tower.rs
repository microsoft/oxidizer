// Copyright (c) Microsoft Corporation.

//! Tower service interoperability for Oxidizer services.
//!
//! This module enables seamless interoperability between Oxidizer's native service trait
//! and the Tower ecosystem of services and middleware. The main adapter type provides
//! bidirectional conversion between service types, allowing:
//!
//! - Tower services to be used as Oxidizer services
//! - Oxidizer services to be used as Tower services
//! - Tower middleware layers to be applied to Oxidizer services
//!
//! **Note**: This tower adapter is particularly useful for Oxidizer-based services that
//! do not implement the Tower `Service` trait natively, enabling them to work seamlessly
//!  with the Tower's rich ecosystem of middleware and utilities.
//!
//! # Adapter Constraints
//!
//! The `Adapter<S>` provides bidirectional conversion with specific constraints for each direction:
//!
//! ## Oxidizer → Tower Direction
//!
//! When wrapping an Oxidizer service to implement Tower's `Service` trait:
//!
//! - **Service**: Must implement `Service<Req, Out = Result<T, E>> + Clone + 'static`
//! - **Types**: Request, response, and error types must be `Send + 'static`
//! - **Behavior**: No backpressure - services are always ready to accept requests
//!
//! ## Tower → Oxidizer Direction
//!
//! When wrapping a Tower service to implement Oxidizer's `Service` trait:
//!
//! - **Service**: Must implement `tower_service::Service<In> + Send + Sync + Clone`
//! - **Future**: The service's future must be `Send`
//! - **Behavior**: Properly handles Tower's backpressure via `poll_ready()`
//!
//! ## Key Requirements
//!
//! - All types must be `Send` for async compatibility
//! - Services must be `Clone` for concurrent usage
//! - Oxidizer services must return `Result<T, E>` when used with Tower
//!
//! # Examples
//!
//! Converting an Oxidizer service to work with Tower middleware:
//!
//! ```rust
//! use std::time::Duration;
//!
//! use layered::tower::Adapter;
//! use layered::{Execute, Service};
//! use tower::ServiceBuilder;
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! // Create an Oxidizer service
//! let service = Execute::new(|req: String| async move {
//!     Ok::<String, std::io::Error>(format!("Processed: {}", req))
//! });
//!
//! // Wrap it for use with Tower middleware
//! let tower_service = ServiceBuilder::new().service(Adapter(service));
//!
//! // The result can be used with Tower's ecosystem
//! # Ok(())
//! # }
//! ```
//!
//! Using Tower middleware with Oxidizer services in an execution stack:
//!
//! ```rust
//! use std::time::Duration;
//!
//! use layered::tower::tower_layer;
//! use layered::{Execute, Service, ServiceBuilder};
//!
//! use tower_layer::Identity;
//!
//! async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! // Create an Oxidizer service
//! let base_service = Execute::new(|req: String| async move {
//!     Ok::<String, std::io::Error>(format!("Processed: {}", req))
//! });
//!
//! let execution_stack = (
//!     tower_layer(Identity::default()), // No-op layer for demonstration
//!     base_service,
//! );
//!
//! // Build the service with the layers applied
//! let service = execution_stack.build();
//!
//! // Use the service
//! let result = service.execute("request".to_string()).await;
//! # Ok(())
//! # }
//! ```

use std::future::poll_fn;
use std::pin::Pin;
use std::task::{Context, Poll};

use tower_layer::Layer;

use crate::Service;

/// Bidirectional adapter between Oxidizer and Tower service traits.
///
/// This adapter automatically implements the appropriate service trait based on the
/// wrapped service type:
/// - When wrapping an Oxidizer service, it implements Tower's [`Service`][tower_service::Service] trait
/// - When wrapping a Tower service, it implements Oxidizer's [`Service`] trait
///
/// The adapter handles the differences between the two service models:
///
/// - Tower services have explicit backpressure via `poll_ready()`
/// - Oxidizer services are always ready and use async execution
/// - Both support cloning for concurrent usage
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
        // Oxidizer services don't have backpressure - they're always ready to accept requests
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
            // Wait for the Tower service to be ready (handle backpressure)
            poll_fn(|cx| clone.poll_ready(cx)).await?;
            // Execute the request
            clone.call(input).await
        }
    }
}

/// Creates a layer that applies Tower layers to Oxidizer services.
///
/// This function provides a convenient way to apply Tower middleware layers
/// to Oxidizer services by wrapping the Tower layer in an adapter that handles
/// the service trait conversions.
///
/// # Examples
///
/// ```rust
/// use layered::tower::tower_layer;
/// use tower::layer::util::Identity;
///
/// // Apply a simple identity layer (no-op for example)
/// let identity_layer = tower_layer(Identity::new());
/// ```
pub fn tower_layer<L>(tower_layer: L) -> AdapterLayer<L> {
    AdapterLayer(tower_layer)
}

/// A layer adapter that applies Tower layers to Oxidizer services.
///
/// This adapter enables Tower middleware layers to be applied to Oxidizer services
/// by handling the necessary service trait conversions. The layer works by:
///
/// 1. Wrapping the Oxidizer service in an `Adapter` to make it Tower-compatible
/// 2. Applying the Tower layer to the adapted service
/// 3. Wrapping the result back in an `Adapter` for Oxidizer compatibility
#[derive(Debug, Clone)]
pub struct AdapterLayer<L>(L);

impl<L, S> Layer<S> for AdapterLayer<L>
where
    L: Layer<Adapter<S>> + Clone,
{
    type Service = Adapter<L::Service>;

    fn layer(&self, inner: S) -> Self::Service {
        // 1. Wrap the Oxidizer service to make it Tower-compatible
        let tower_adapted = Adapter(inner);
        // 2. Apply the Tower layer
        let tower_layered = self.0.layer(tower_adapted);
        // 3. Wrap the result back for Oxidizer compatibility
        Adapter(tower_layered)
    }
}

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

        // Multiple calls should return the same result
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
        // This test specifically addresses the mutation testing result
        // Both Poll::Ready(Ok(())) and Poll::from(Ok(())) should be functionally equivalent
        let mock_service = MockService::new(Poll::Ready(Ok(())), Ok("success".to_string()));
        let mut adapter = Adapter(mock_service);

        let waker = futures::task::noop_waker();
        let mut cx = Context::from_waker(&waker);

        let result1 = adapter.poll_ready(&mut cx);
        let result2 = Poll::from(Ok::<(), String>(()));

        // Both should be Poll::Ready(Ok(()))
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
}
