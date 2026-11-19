// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use core::fmt;

/// The fixed role of a driver on its async worker.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DriverRole {
    /// The driver that can wait for I/O on its async worker.
    ///
    /// An async worker has at most one primary. It runs after all secondaries and can wait for up
    /// to [`Cycle::max_wait`](crate::Cycle::max_wait).
    Primary,
    /// A driver that must not wait for I/O on its async worker.
    ///
    /// The runtime invokes this driver before the primary with
    /// `Cycle::max_wait() == Duration::ZERO`. The driver must arrange notification when another
    /// cycle is necessary, unless a background thread completes all operations.
    Secondary,
}

impl fmt::Display for DriverRole {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Primary => "primary",
            Self::Secondary => "secondary",
        })
    }
}
