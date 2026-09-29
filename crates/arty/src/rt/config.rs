// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Runtime construction and resource configuration.
//!
//! Start with [`Runtime::builder`](crate::rt::Runtime::builder) to choose resource
//! limits, clocks, and telemetry before starting the runtime.

pub use crate::rt::runtime::builder::RuntimeBuilder;
pub use crate::rt::runtime::config::{ProcessorCount, WorkerPoolPolicy};
pub use crate::rt::runtime::error::BuildError;
