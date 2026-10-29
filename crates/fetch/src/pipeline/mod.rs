// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Request pipelines for [`HttpClient`](crate::HttpClient).
//!
//! A pipeline defines its handlers and their order. The client supports three types:
//!
//! - Standard includes timeouts, retries, logging, and metrics. Configure it through
//!   [`StandardRequestPipeline`][crate::pipeline::StandardRequestPipeline].
//! - Custom uses a caller-defined stack above the dispatch handler.
//! - Minimal contains only the dispatch handler.
//!
//! [`PipelineContext`][crate::pipeline::PipelineContext] gives pipeline factories the clock, meter,
//! router, and other shared dependencies. [`HttpClientBuilder`](crate::HttpClientBuilder) selects
//! the pipeline type.

mod builder;
mod custom;
mod standard;

mod pipeline_context;

pub(crate) use builder::{Pipeline, PipelineBuilder};
pub(crate) use custom::CustomPipeline;
pub use pipeline_context::PipelineContext;
pub use standard::{RecoveryMode, StandardRequestPipeline};
