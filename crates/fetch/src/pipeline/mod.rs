// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Request-processing pipelines that wire handlers together for an [`HttpClient`](crate::HttpClient).
//!
//! A pipeline determines which handlers a request passes through, and in what
//! order. The client supports three flavors:
//!
//! - **standard**: a production-ready stack with timeouts, retries, logging, and
//!   metrics, configured via [`StandardRequestPipeline`].
//! - **custom**: a fully user-defined stack of layers over the dispatch handler.
//! - **minimal**: only the dispatch handler, with no middleware.
//!
//! [`PipelineContext`] carries the shared dependencies (clock, meter, router, and
//! so on) handed to pipeline factories. See
//! [`HttpClientBuilder`](crate::HttpClientBuilder) for how each flavor is selected.

mod builder;
mod custom;
mod standard;

mod pipeline_context;

pub(crate) use builder::{Pipeline, PipelineBuilder};
pub(crate) use custom::CustomPipelineFactory;
pub use pipeline_context::PipelineContext;
pub use standard::{RecoveryMode, StandardRequestPipeline};
