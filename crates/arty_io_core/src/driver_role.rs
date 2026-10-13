// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

/// A driver's fixed waiting role on its runtime worker.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DriverRole {
    /// The driver permitted to block its runtime worker.
    ///
    /// A worker has at most one primary. It runs after all secondaries and may wait for up to
    /// [`Cycle::max_wait`](crate::Cycle::max_wait). A zero wait bound means no waiting.
    Primary,
    /// A driver whose worker-local cycle must not block.
    ///
    /// The runtime invokes this driver before the primary, and it must not wait on the worker.
    ///
    /// Indefinite waits require independent execution capacity for every simultaneously blocked
    /// observer.
    Secondary,
}
