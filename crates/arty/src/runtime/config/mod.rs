// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Resource limits and worker-pool policy, independent of live runtime state.

mod blocking_pool_policy;
mod monitor_options;
mod monitor_policy;
mod panic_policy;
mod runtime_config;
mod runtime_name;
mod workers_policy;

pub use blocking_pool_policy::BlockingPoolPolicy;
pub(crate) use monitor_options::MonitorOptions;
pub use monitor_policy::MonitorPolicy;
pub use panic_policy::PanicPolicy;
pub(crate) use runtime_config::RuntimeConfig;
pub(crate) use runtime_name::RuntimeName;
pub use workers_policy::WorkersPolicy;
