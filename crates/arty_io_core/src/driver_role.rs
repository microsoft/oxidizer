// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

/// A driver's fixed role on its async worker.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DriverRole {
    /// The driver permitted to wait for I/O on its async worker.
    ///
    /// An async worker has at most one primary. It runs after all secondaries and may wait for up
    /// to [`Cycle::max_wait`](crate::Cycle::max_wait).
    Primary,
    /// A driver that must not wait for I/O on its async worker.
    ///
    /// The runtime invokes this driver before the primary with
    /// `Cycle::max_wait() == Duration::ZERO`. The driver must arrange notification when another
    /// cycle is needed, unless it completes operations entirely on a background thread.
    Secondary,
}
