// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::time::{Duration, SystemTime};

/// Represents a type of timestamp used by [`Clock`][crate::Clock] and [`ClockControl`][crate::ClockControl].
#[derive(Debug)]
#[non_exhaustive]
pub enum ClockTimestamp {
    /// Represents system time.
    System(SystemTime),
    /// Represents a specific timestamp.
    Timestamp(crate::Timestamp),
    /// Represents an offset duration from the UNIX epoch.
    Offset(Duration),
}

impl From<SystemTime> for ClockTimestamp {
    fn from(time: SystemTime) -> Self {
        Self::System(time)
    }
}

impl From<Duration> for ClockTimestamp {
    fn from(duration: Duration) -> Self {
        Self::Offset(duration)
    }
}

#[cfg(any(feature = "timestamp", test))]
impl<T: Into<crate::Timestamp>> From<T> for ClockTimestamp {
    fn from(timestamp: T) -> Self {
        Self::Timestamp(timestamp.into())
    }
}
