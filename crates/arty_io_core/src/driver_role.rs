// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

/// A driver's fixed waiting role on its runtime worker.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DriverRole {
    /// The driver permitted to block its runtime worker.
    ///
    /// A worker has at most one primary. The runtime grants permission to select this role
    /// through [`DriverOptions::allowed_roles`](crate::DriverOptions::allowed_roles). It runs
    /// after all secondaries and may wait for up to [`Cycle::max_wait`](crate::Cycle::max_wait).
    /// A zero wait bound means no waiting.
    Primary,
    /// A driver whose worker-local cycle must not block.
    ///
    /// The runtime invokes this driver before the primary.
    /// The driver must not block its worker. It may coordinate with another driver or process
    /// completions continuously on a driver-owned background thread.
    Secondary,
}
