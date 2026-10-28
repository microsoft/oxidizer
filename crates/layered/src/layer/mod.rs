// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

mod stack;
mod tuples;

#[doc(inline)]
pub use stack::Stack;
/// Wraps a [`Service`](crate::Service) with middleware.
///
/// A `Layer` takes an inner service and returns a service with more behavior.
/// Multiple layers can form a processing pipeline.
///
/// # Tower compatibility
///
/// > **Note**: This crate re-exports this trait from
/// > [`tower_layer`](https://docs.rs/tower-layer).
///
/// A layer can work with either service system. The resulting service must implement
/// [`Service`](crate::Service) or
/// [`tower_service::Service`](https://docs.rs/tower-service/latest/tower_service/trait.Service.html).
/// A middleware type can implement both service traits.
///
/// Enable the `tower-service` feature for more Tower adapters.
///
/// # Behavior
///
/// The `layer` method wraps an inner service `S`. It returns `Self::Service`, which can add:
///
/// - Logging and tracing.
/// - Timeouts and retries.
/// - Authentication and authorization.
/// - Rate limits.
/// - Metrics.
///
/// # Upstream documentation
///
/// The re-export also includes documentation from
/// [`tower_layer`](https://docs.rs/tower-layer). That documentation refers to
/// [`tower::Service`](https://docs.rs/tower/latest/tower/trait.Service.html).
/// This crate defines a different [`Service`](crate::Service) trait.
///
/// ---
pub use tower_layer::Layer;
