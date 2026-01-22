// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![cfg_attr(
    not(all(feature = "intercept", feature = "tower-service", feature = "dynamic-service")),
    expect(rustdoc::broken_intra_doc_links)
)]
#![doc(html_logo_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/layered/logo.png")]
#![doc(html_favicon_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/layered/favicon.ico")]

//! # Layered Services
//!
//! Build composable async services with layered middleware.
//!
//! This crate provides the [`Service`] trait and a layer system for adding cross-cutting
//! concerns like timeouts, retries, and logging.
//!
//! ## Why not Tower?
//!
//! [Tower](https://docs.rs/tower) predates `async fn` in traits, requiring manual `Future` types
//! or boxing and `poll_ready` back-pressure semantics. Tower's `&mut self` also requires cloning
//! for concurrent requests. This crate uses `async fn` with `&self`, enabling simpler middleware
//! and natural concurrency. Tower interop is available via the `tower-service` feature.
//!
//! ## Quick Start
//!
//! A [`Service`] transforms an input into an output asynchronously:
//!
//! ```
//! use layered::Service;
//!
//! struct Greeter;
//!
//! impl Service<String> for Greeter {
//!     type Out = String;
//!
//!     async fn execute(&self, name: String) -> Self::Out {
//!         format!("Hello, {name}!")
//!     }
//! }
//! ```
//!
//! Use [`Execute`] to turn any async function into a service:
//!
//! ```
//! use layered::{Execute, Service};
//!
//! # async fn example() {
//! let greeter = Execute::new(|name: String| async move {
//!     format!("Hello, {name}!")
//! });
//!
//! assert_eq!(greeter.execute("World".into()).await, "Hello, World!");
//! # }
//! ```
//!
//! ## Key Concepts
//!
//! - **Service**: A type implementing the [`Service`] trait that transforms inputs into outputs
//!   asynchronously. Think of it as `async fn(&self, In) -> Out`.
//! - **Middleware**: A service that wraps another service to add cross-cutting behavior such as
//!   logging, timeouts, or retries. Middleware receives requests before the inner service and can
//!   process responses after.
//! - **Layer**: A type implementing the [`Layer`] trait that constructs middleware around a
//!   service. Layers are composable and can be stacked using tuples like `(layer1, layer2, service)`.
//!
//! ## Layers and Middleware
//!
//! A [`Layer`] wraps a service with additional behavior. In this example, we create a logging
//! middleware that prints inputs before passing them to the inner service:
//!
//! ```
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
//! impl<S, In: Send + std::fmt::Display> Service<In> for LogService<S>
//! where
//!     S: Service<In>,
//! {
//!     type Out = S::Out;
//!
//!     async fn execute(&self, input: In) -> Self::Out {
//!         println!("Input: {input}");
//!         self.0.execute(input).await
//!     }
//! }
//!
//! # async fn example() {
//! // Stack layers with the service (layers apply outer to inner)
//! let service = (
//!     LogLayer,
//!     Execute::new(|x: i32| async move { x * 2 }),
//! ).into_service();
//!
//! let result = service.execute(21).await;
//! # }
//! ```
//!
//! ## Thread Safety
//!
//! All services must implement [`Send`] and [`Sync`], and returned futures must be [`Send`].
//! This ensures compatibility with multi-threaded async runtimes like Tokio.
//!
//! ## Features
//!
//! - **`intercept`**: Enables [`Intercept`] middleware
//! - **`dynamic-service`**: Enables [`DynamicService`] for type erasure
//! - **`tower-service`**: Enables Tower interoperability via the [`tower`] module

mod service;
pub use service::Service;

mod execute;
pub use execute::Execute;

mod layer;
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
