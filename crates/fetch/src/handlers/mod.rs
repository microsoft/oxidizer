// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Composable handlers that process HTTP requests as they flow through the pipeline.
//!
//! Each handler adds one specific behavior — buffering, metrics, logging, or
//! network dispatch — and wraps the next handler in the chain. Stacking them
//! builds the request-processing pipeline used by an
//! [`HttpClient`](crate::HttpClient).
//!
//! See [`RequestHandler`][super::RequestHandler] for how handlers work and how to
//! write your own.
//!
//! # Available Handlers
//!
//! - [`Buffering`]: buffers the entire response body into memory.
//! - [`Metrics`]: collects performance data for monitoring.
//! - [`Logging`]: adds structured request/response logging.
//! - [`Dispatch`]: sends requests to the network (managed by the `HttpClient`).

mod dispatch;
pub use dispatch::Dispatch;
pub(crate) use dispatch::DispatchMode;

mod metrics;
pub use metrics::{Metrics, MetricsLayer};

mod logging;
pub use logging::{Logging, LoggingLayer};

mod buffering;
pub use buffering::{Buffering, BufferingLayer};

mod transport;
pub(crate) use transport::TransportHandler;
