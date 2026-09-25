// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

/// A driver's fixed runtime-assigned waiting role on one worker.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DriverRole {
    /// The single driver whose cycle may block the runtime worker.
    ///
    /// A worker has at most one primary. The runtime assigns this role only to a provider whose
    /// [`DriverProvider::CAN_BE_PRIMARY`](crate::DriverProvider::CAN_BE_PRIMARY) flag is true,
    /// invokes it after every secondary, and permits it to apply
    /// [`Cycle::max_wait`](crate::Cycle::max_wait) directly to its worker wait.
    Primary,
    /// A driver whose worker-local cycle must remain non-blocking.
    ///
    /// The runtime invokes a secondary before the primary so it can arm off-worker observation
    /// before the worker may block. It receives the same [`Cycle::max_wait`](crate::Cycle::max_wait)
    /// as the primary; that value is a timeout for an off-worker wait, not permission to block or
    /// join from [`Driver::execute_cycle`](crate::Driver::execute_cycle). It claims the cycle's
    /// [`PendingWork`](crate::PendingWork) value for background work and completes that
    /// value before the runtime advances. If the observer publishes work, it calls
    /// [`PendingWork::complete`](crate::PendingWork::complete); otherwise it
    /// drops the value without interrupting the cycle.
    Secondary,
}
