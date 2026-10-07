// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Resource limits and worker-pool policy, independent of live runtime state.

mod blocking_pool_policy;
mod workers_policy;

pub use blocking_pool_policy::BlockingPoolPolicy;
pub(crate) use workers_policy::RuntimeConfig;
pub use workers_policy::WorkersPolicy;
