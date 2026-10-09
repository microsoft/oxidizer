// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

/// A driver's fixed waiting role on its runtime worker.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DriverRole {
    /// The driver permitted to block its runtime worker.
    ///
    /// A worker has at most one primary, assigned only when
    /// [`DriverProvider::CAN_BE_PRIMARY`](crate::DriverProvider::CAN_BE_PRIMARY) is `true`.
    /// It runs after all secondaries and may wait for up to [`Cycle::max_wait`](crate::Cycle::max_wait).
    /// A zero wait bound means no waiting.
    Primary,
    /// A driver whose worker-local cycle must not block.
    ///
    /// The runtime invokes this driver before the primary.
    /// The driver may apply [`Cycle::max_wait`](crate::Cycle::max_wait) only to background waits
    /// registered through [`Cycle::start_work`](crate::Cycle::start_work).
    ///
    /// Indefinite waits require independent execution capacity. A bounded pool behind
    /// [`SystemTaskSpawner`](crate::SystemTaskSpawner) is suitable only if it has capacity
    /// for every simultaneously blocked observer.
    Secondary,
}
