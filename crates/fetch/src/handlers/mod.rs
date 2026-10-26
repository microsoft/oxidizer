// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Handlers that process HTTP requests in a pipeline.
//!
//! Each handler adds one behavior, such as buffering, metrics, logging, or dispatch. It wraps the
//! next handler in the chain. Together, the handlers form the pipeline for
//! [`HttpClient`](crate::HttpClient).
//!
//! See [`RequestHandler`][crate::RequestHandler] for how handlers work and how to
//! write your own.
//!
//! # Available handlers
//!
//! - [`Buffering`][crate::handlers::Buffering] buffers the response body in memory.
//! - [`Metrics`][crate::handlers::Metrics] records HTTP metrics.
//! - [`Logging`][crate::handlers::Logging] records structured request and response events.
//! - [`Dispatch`][crate::handlers::Dispatch] sends requests through the transport.

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
