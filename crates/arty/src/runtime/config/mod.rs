// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Resource limits and worker-pool policy, independent of live runtime state.

mod blocking_pool_policy;
mod processors;

pub use blocking_pool_policy::BlockingPoolPolicy;
pub use processors::ProcessorCount;
pub(crate) use processors::RuntimeConfig;
