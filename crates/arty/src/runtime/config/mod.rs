// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Resource limits and worker-pool policy, independent of live runtime state.

mod processors;
mod worker_pool_policy;

pub use processors::ProcessorCount;
pub(crate) use processors::RuntimeConfig;
pub use worker_pool_policy::WorkerPoolPolicy;
