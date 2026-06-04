// Copyright (c) Microsoft Corporation.

//! # HTTP Request Handlers
//!
//! HTTP requests flow through a pipeline of handlers that each add specific functionality.
//! Stack them together to build your custom request processing chain!
//!
//! See [`RequestHandler`][super::RequestHandler] for more details on how handlers work and how to create your own.
//!
//! ## Available Handlers
//!
//! - [`Buffering`]: Buffers the entire response body into memory
//! - [`Metrics`]: Collects performance data for monitoring
//! - [`Logging`]: Adds structured request/response logging
//! - [`Dispatch`]: Sends requests to the network (managed by the `HttpClient`)

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
