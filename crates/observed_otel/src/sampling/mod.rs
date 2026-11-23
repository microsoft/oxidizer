// Copyright (c) Microsoft Corporation.

//! Log sampling after enrichment and redaction.
//!
//! Attach a [`LogSampler`] through [`crate::LogDestinationExt::with_sampler`].
//! A returned [`SamplingBatch`] is sent to that sampler's destination.

mod batch;
mod log;

pub use batch::SamplingBatch;
pub use log::{LogSampler, LogSamplingContext};
