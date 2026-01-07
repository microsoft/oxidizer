// Copyright (c) Microsoft Corporation.

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![doc(html_logo_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/layered/logo.png")]
#![doc(html_favicon_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/layered/favicon.ico")]

//! # Layered Services
//!
//! Build composable async services with stackable middleware.
//!
//! This crate provides the [`Service`] trait and a layer system for adding cross-cutting
//! concerns like timeouts, retries, and logging.
//!
//! ## Why not Tower?
//!
//! [Tower](https://docs.rs/tower) predates `async fn` in traits, requiring manual `Future` types
//! or boxing and `poll_ready` backpressure semantics. Tower's `&mut self` also requires cloning
//! for concurrent requests. This crate uses `async fn` with `&self`, enabling simpler middleware
//! and natural concurrency. Tower interop is available via the `tower-service` feature.
//!
//! ## Quickstart
//!
//! A [`Service`] transforms an input into an output asynchronously:
//!
//! ```rust
//! use layered::Service;
//!
//! struct Greeter;
//!
//! impl Service<String> for Greeter {
//!     type Out = String;
//!
//!     async fn execute(&self, name: String) -> Self::Out {
//!         format!("Hello, {}!", name)
//!     }
//! }
//! ```
//!
//! Use [`Execute`] to turn any async function into a service:
//!
//! ```rust
//! use layered::{Execute, Service};
//!
//! # async fn example() {
//! let greeter = Execute::new(|name: String| async move {
//!     format!("Hello, {}!", name)
//! });
//!
//! assert_eq!(greeter.execute("World".into()).await, "Hello, World!");
//! # }
//! ```
//!
//! ## Key Concepts
//!
//! - **Service**: An async function `In → Out` that processes inputs.
//! - **Middleware**: A service that wraps another service to add behavior (logging, timeouts, retries).
//! - **Layer**: A factory that wraps any service with middleware. Stack layers using tuples
//!   like `(layer1, layer2, service)`.
//!
//! ## Layers and Middleware
//!
//! A [`Layer`] wraps a service with additional behavior:
//!
//! ```rust
//! use layered::{Execute, Layer, Service, Stack};
//!
//! // A simple logging layer
//! struct LogLayer;
//!
//! impl<S> Layer<S> for LogLayer {
//!     type Service = LogService<S>;
//!
//!     fn layer(&self, inner: S) -> Self::Service {
//!         LogService(inner)
//!     }
//! }
//!
//! struct LogService<S>(S);
//!
//! impl<S, In: Send + std::fmt::Debug> Service<In> for LogService<S>
//! where
//!     S: Service<In>,
//! {
//!     type Out = S::Out;
//!
//!     async fn execute(&self, input: In) -> Self::Out {
//!         println!("Input: {:?}", input);
//!         self.0.execute(input).await
//!     }
//! }
//!
//! # async fn example() {
//! // Stack layers with the service (layers apply outer to inner)
//! let service = (
//!     LogLayer,
//!     Execute::new(|x: i32| async move { x * 2 }),
//! ).build();
//!
//! let result = service.execute(21).await;
//! # }
//! ```
//!
//! ## Thread Safety
//!
//! All services must be [`Send`] + [`Sync`], and returned futures must be [`Send`].
//! This ensures compatibility with multi-threaded async runtimes like Tokio.
//!
//! ## Features
//!
//! - **`intercept`** — Enables [`Intercept`] middleware
//! - **`dynamic-service`** — Enables [`DynamicService`] for type erasure
//! - **`tower-service`** — Enables Tower interoperability via the [`tower`] module

mod service;
pub use service::Service;

mod execute;
pub use execute::Execute;

mod layer;
#[doc(inline)]
pub use layer::{Layer, Stack};

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

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
pub(crate) mod testing;
