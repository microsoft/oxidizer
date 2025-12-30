// Copyright (c) Microsoft Corporation.

//! # Service Composition Framework
//!
//! A foundational service abstraction for building composable, middleware-driven systems.
//! This crate provides the [`Service`] trait and layer composition system that enables systematic
//! application of cross-cutting concerns such as timeouts, retries, and observability.
//!
//! # Why
//!
//! This crate enables easy composability using modern Rust features. Why not just use
//! [Tower](https://docs.rs/tower)? Tower predates `async fn` in traits, requiring boxed futures and
//! `poll_ready` semantics that add complexity we don't need. This crate provides a simpler
//! `execute`-based model with cleaner trait bounds, while still offering Tower interoperability
//! via the `tower-service` feature.
//!
//! # Quickstart
//!
//! A basic service transforms an input into an output:
//!
//! ```rust
//! use std::future::Future;
//!
//! use layered::Service;
//!
//! struct DatabaseService;
//!
//! impl Service<String> for DatabaseService {
//!     type Out = Vec<u8>;
//!
//!     async fn execute(&self, query: String) -> Self::Out {
//!         // Simulate database query execution
//!         format!("SELECT * FROM users WHERE name = '{}'", query).into_bytes()
//!     }
//! }
//! ```
//!
//! ## Key Concepts
//!
//! - **Service**: An async function `In → Future<Out>` that processes inputs. All services
//!   implement the [`Service`] trait.
//! - **Middleware**: A service wrapper that adds cross-cutting functionality (logging, timeouts, retries)
//!   before delegating to an inner service. Middleware also implements the [`Service`] trait.
//! - **Layer**: A factory that wraps any service with middleware functionality. Multiple layers can
//!   be combined using tuple syntax like `(timeout, retry, core_service)` to create an execution stack
//!   where middleware is applied in order, with the core service at the bottom.
//!
//! ## Middleware
//!
//! Services can be composed by wrapping them with additional services. Middleware services
//! add functionality such as logging, metrics, or error handling, then call the inner service.
//!
//! ```rust
//! use layered::Service;
//!
//! struct Logging<S> {
//!     inner: S,
//!     name: &'static str,
//! }
//!
//! impl<S, In: Send> Service<In> for Logging<S>
//! where
//!     S: Service<In>,
//!     In: std::fmt::Debug,
//!     S::Out: std::fmt::Debug,
//! {
//!     type Out = S::Out;
//!
//!     async fn execute(&self, input: In) -> Self::Out {
//!         println!("{}: Processing input: {:?}", self.name, input);
//!         let output = self.inner.execute(input).await;
//!         println!("{}: Output: {:?}", self.name, output);
//!         output
//!     }
//! }
//! ```
//!
//! # Layer and Composition
//!
//! For systematic middleware composition, use the [`crate::Layer`]. Layers are builders
//! for middleware services that can be applied to any service. This allows you to create reusable
//! and composable middleware.
//!
//! ```rust
//! use layered::{Execute, Layer, Service, ServiceBuilder};
//!
//! // The middleware service
//! pub struct Timeout<S> {
//!     inner: S,
//!     timeout: std::time::Duration,
//! }
//!
//! impl Timeout<()> {
//!     // By convention, layers are created using a `layer` method exposed
//!     // by the middleware.
//!     pub fn layer(timeout: std::time::Duration) -> TimeoutLayer {
//!         TimeoutLayer { timeout }
//!     }
//! }
//!
//! // Middleware implements the `Service` trait
//! impl<S, In: Send> Service<In> for Timeout<S>
//! where
//!     S: Service<In>,
//! {
//!     type Out = Result<S::Out, &'static str>;
//!
//!     async fn execute(&self, input: In) -> Self::Out {
//!         // In a real implementation, this would use a proper timeout mechanism
//!         Ok(self.inner.execute(input).await)
//!     }
//! }
//!
//! // Actual layer that is able to wrap inner service with logging functionality
//! pub struct TimeoutLayer {
//!     timeout: std::time::Duration,
//! }
//!
//! // Layer must be implemented
//! impl<S> Layer<S> for TimeoutLayer {
//!     type Service = Timeout<S>;
//!
//!     fn layer(&self, inner: S) -> Self::Service {
//!         Timeout {
//!             inner,
//!             timeout: self.timeout,
//!         }
//!     }
//! }
//!
//! # async fn sample() {
//!
//! // Define the layers and the root service
//! let execution_stack = (
//!     Timeout::layer(std::time::Duration::from_secs(5)),
//!     Execute::new(|input: String| async move { input }),
//! );
//!
//! // Build the service with the layers applied
//! let service = execution_stack.build();
//!
//! // Execute an input
//! let output = service.execute("hello".to_string()).await;
//! # }
//! ```
//!
//! # Thread Safety and Concurrency
//!
//! All [`Service`] implementations must be [`Send`] and [`Sync`], enabling safe use across
//! threads and async runtimes. This is essential because:
//!
//! - **Multi-threaded runtimes**: Services may be called from different threads in runtimes such as Tokio
//! - **Concurrent inputs**: Multiple inputs may be processed simultaneously using the same service
//! - **Shared state**: Services are often shared between different parts of an application
//!
//! The returned future must also be [`Send`], ensuring it can be moved between threads during
//! async execution. This enables services to work seamlessly in both single-threaded
//! (thread-per-core) and multi-threaded runtime environments.
//!
//! # Built-in Services and Middleware
//!
//! - **[`Execute`]**: Converts any function or closure into a service. Always available.
//! - **`Intercept`**: Middleware for observing and modifying service inputs and outputs.
//!   Useful for logging, debugging, and validation. Requires the `intercept` feature.
//! - **`DynamicService`**: Type-erased service wrapper for hiding concrete service types.
//!   Useful for complex compositions and collections. Requires the `dynamic-service` feature.
//!
//! # Tower Service Interoperability
//!
//! This crate provides seamless interoperability with the Tower ecosystem through the `tower-service` feature.
//! When enabled, you can:
//!
//! - Convert between oxidizer [`Service`] and Tower's `tower::Service` trait
//! - Use existing Tower middleware with oxidizer services
//! - Integrate oxidizer services into Tower-based applications
//!
//! The `tower` module contains all Tower-related functionality and is only available
//! when the `tower-service` feature is enabled.
//!
//! # Features
//!
//! This crate supports the following optional features:
//!
//! - **`intercept`**: Enables the `Intercept` middleware for debugging and observability
//! - **`dynamic-service`**: Enables `DynamicService` and `DynamicServiceExt` for type-erased services
//! - **`tower-service`**: Enables interoperability with the Tower ecosystem via the `tower` module

mod service;
pub use service::Service;

mod execute;
pub use execute::Execute;

mod layer;
#[doc(inline)]
pub use layer::{Layer, ServiceBuilder};

#[cfg(any(test, feature = "dynamic-service"))]
mod dynamic;

#[cfg(any(test, feature = "dynamic-service"))]
pub use dynamic::{DynamicService, DynamicServiceExt};

pub mod prelude;

#[cfg(any(test, feature = "intercept"))]
mod intercept;
#[doc(inline)]
#[cfg(any(test, feature = "intercept"))]
pub use intercept::{Intercept, InterceptLayer};

#[cfg(any(test, feature = "tower-service"))]
pub mod tower;

#[cfg(test)]
pub mod testing;
