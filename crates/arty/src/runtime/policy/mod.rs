// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Policies for configuring a runtime before it starts.
//!
//! Use [`RuntimeBuilder`](crate::runtime::RuntimeBuilder) to select async workers,
//! configure the blocking pool, or enable or disable monitoring.

mod blocking_pool_policy;
mod monitor_policy;
mod workers_policy;

#[doc(inline)]
pub use blocking_pool_policy::BlockingPoolPolicy;
#[doc(inline)]
pub use monitor_policy::MonitorPolicy;
#[doc(inline)]
pub use workers_policy::WorkersPolicy;
