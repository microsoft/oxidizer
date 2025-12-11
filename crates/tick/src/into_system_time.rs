// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::time::{Duration, SystemTime};

/// A trait for types that can be converted into a [`SystemTime`].
///
/// This trait is used by [`ClockControl`][crate::ClockControl] to accept various
/// timestamp representations when setting the clock's timestamp.
pub trait IntoSystemTime: crate::sealed::Sealed {
    /// Converts this value into a [`SystemTime`].
    fn into_system_time(self) -> SystemTime;
}

impl IntoSystemTime for SystemTime {
    fn into_system_time(self) -> SystemTime {
        self
    }
}

impl IntoSystemTime for Duration {
    fn into_system_time(self) -> SystemTime {
        SystemTime::UNIX_EPOCH + self
    }
}

#[cfg_attr(docsrs, doc(cfg(feature = "timestamp")))]
#[cfg(any(feature = "timestamp", test))]
impl IntoSystemTime for crate::Timestamp {
    fn into_system_time(self) -> SystemTime {
        self.to_system_time()
    }
}
