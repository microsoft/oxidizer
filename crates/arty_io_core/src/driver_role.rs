// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

/// A driver's fixed runtime-assigned waiting role on one worker.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DriverRole {
    /// The single driver whose cycle may block the runtime worker.
    ///
    /// A worker that hosts drivers has exactly one primary. The runtime invokes it after every
    /// secondary. It may apply [`Cycle::max_wait`](crate::Cycle::max_wait) directly to its
    /// worker wait.
    Primary,
    /// A driver whose worker-local cycle must remain non-blocking.
    ///
    /// The runtime invokes a secondary before the primary so it can arm off-worker observation
    /// before the worker may block. It receives the same [`Cycle::max_wait`](crate::Cycle::max_wait)
    /// as the primary; that value is a timeout for an off-worker wait, not permission to block or
    /// join from [`Driver::execute_cycle`](crate::Driver::execute_cycle). It claims the cycle's
    /// [`CoordinationToken`](crate::CoordinationToken) for background work and completes that
    /// token before the runtime advances. If the observer publishes work, it calls
    /// [`CoordinationToken::work_ready`](crate::CoordinationToken::work_ready); otherwise it
    /// drops the token without interrupting the cycle.
    Secondary,
}
