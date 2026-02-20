// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

mod stack;
mod tuples;

#[doc(inline)]
pub use stack::Stack;
/// A trait for decorating a [`Service`](crate::Service) with middleware.
///
/// A `Layer` takes an inner service and wraps it with additional behavior,
/// producing a new service. This pattern enables building reusable middleware
/// components that can be composed together to form a processing pipeline.
///
/// # Tower Ecosystem Compatibility
///
/// **Important:** This trait is re-exported from [`tower_layer`](https://docs.rs/tower-layer)
/// to ensure seamless interoperability with the Tower ecosystem.
///
/// Layers built in the Tower ecosystem or using this crate can work in both ecosystems,
/// provided the resulting service implements either [`layered::Service`](crate::Service) or
/// [`tower_service::Service`](https://docs.rs/tower-service/latest/tower_service/trait.Service.html).
/// Middleware authors can also choose to implement both service traits for seamless
/// interoperability across both ecosystems.
///
/// See also the `tower-service` feature in this crate for Tower interoperability utilities.
///
/// # How It Works
///
/// The `layer` method wraps an inner service `S` and returns a new service
/// (`Self::Service`) that adds cross-cutting behavior such as:
///
/// - Logging and tracing
/// - Timeouts and retries
/// - Authentication and authorization
/// - Rate limiting
/// - Metrics collection
///
/// # Original Documentation
///
/// The documentation below is from the original [`tower_layer`](https://docs.rs/tower-layer)
/// crate. Note that it references [`tower::Service`](https://docs.rs/tower/latest/tower/trait.Service.html),
/// whereas this crate uses its own [`Service`](crate::Service) trait. The concepts
/// remain the same, but the service trait definition differs.
///
/// ---
pub use tower_layer::Layer;
