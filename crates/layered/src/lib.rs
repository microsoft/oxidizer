// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(all(coverage_nightly, test), feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![cfg_attr(
    not(all(feature = "intercept", feature = "tower-service", feature = "dynamic-service")),
    expect(rustdoc::broken_intra_doc_links, reason = "optional items are absent without all features")
)]
#![doc(html_logo_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/layered/logo.png")]
#![doc(html_favicon_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/layered/favicon.ico")]

//! # Layered services
//!
//! Build composable async services with layered middleware.
//!
//! The [`Service`] trait defines an async operation. Layers wrap services to add behavior
//! such as timeouts, retries, and logging.
//!
//! ## Why not Tower?
//!
//! [Tower](https://docs.rs/tower) predates `async fn` in traits. Its services use explicit
//! `Future` types and `poll_ready` for back-pressure. They also use `&mut self`, so concurrent
//! requests can require service clones. This crate uses `async fn` with `&self`.
//! Enable the `tower-service` feature to use the Tower service adapters.
//!
//! ## Quick start
//!
//! A [`Service`] asynchronously transforms an input into an output:
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
//! let greeter = Execute::new(|name: String| async move { format!("Hello, {name}!") });
//!
//! assert_eq!(greeter.execute("World".into()).await, "Hello, World!");
//! # }
//! ```
//!
//! ## Key concepts
//!
//! - A [`Service`] is similar to `async fn(&self, In) -> Out`.
//! - Middleware wraps a service to add behavior before or after execution.
//! - A [`Layer`] creates middleware around a service.
//!
//! ## Layers and middleware
//!
//! A [`Layer`] wraps a service with more behavior. This layer prints each input before it calls
//! the inner service:
//!
//! ```
//! use layered::{Execute, Layer, Service, Stack};
//!
//! // Define a layer that adds logging.
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
//! // The first tuple element becomes the outer layer.
//! let service = (LogLayer, Execute::new(|x: i32| async move { x * 2 })).into_service();
//!
//! let result = service.execute(21).await;
//! # }
//! ```
//!
//! ## Thread safety
//!
//! Each service must implement [`Send`] and [`Sync`]. Each returned future must implement
//! [`Send`]. These bounds support multithreaded async runtimes such as Tokio.
//!
//! ## Features
//!
//! - `intercept` enables [`Intercept`] middleware.
//! - `dynamic-service` enables the type-erased [`DynamicService`].
//! - `tower-service` enables Tower support through the [`tower`] module.

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
