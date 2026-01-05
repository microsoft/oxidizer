// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::time::SystemTime;

/// Extension trait for [`SystemTime`] that provides formatting capabilities.
pub trait SystemTimeExt: sealed::Sealed {
    /// Returns a value that formats the [`SystemTime`] in ISO 8601 format.
    ///
    /// Times outside the valid range (before year -9999 or after year 9999) are saturated
    /// to the nearest boundary.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::time::{Duration, SystemTime};
    /// use tick::SystemTimeExt;
    ///
    /// let time = SystemTime::UNIX_EPOCH + Duration::from_secs(3600);
    /// assert_eq!(time.display_iso_8601().to_string(), "1970-01-01T01:00:00Z");
    /// ```
    #[cfg(any(feature = "fmt", test))]
    fn display_iso_8601(&self) -> impl std::fmt::Display;
}

impl SystemTimeExt for SystemTime {
    #[cfg(any(feature = "fmt", test))]
    fn display_iso_8601(&self) -> impl std::fmt::Display {
        // jiff's Timestamp implements Display that outputs ISO 8601 format
        to_timestamp_saturating(*self)
    }
}

#[cfg(any(feature = "fmt", test))]
fn to_timestamp_saturating(system_time: SystemTime) -> jiff::Timestamp {
    use jiff::Timestamp;

    match Timestamp::try_from(system_time) {
        Ok(timestamp) => timestamp,
        Err(_) => {
            if after_unix_epoch(system_time) {
                Timestamp::MAX
            } else {
                Timestamp::MIN
            }
        }
    }
}

#[cfg(any(feature = "fmt", test))]
fn after_unix_epoch(system_time: SystemTime) -> bool {
    system_time.duration_since(SystemTime::UNIX_EPOCH).is_ok()
}

mod sealed {
    pub trait Sealed {}
    impl Sealed for std::time::SystemTime {}
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use std::time::Duration;

    use jiff::Timestamp;

    use super::*;

    #[test]
    fn display_ok() {
        assert_eq!(SystemTime::UNIX_EPOCH.display_iso_8601().to_string(), "1970-01-01T00:00:00Z");

        assert_eq!(
            (SystemTime::UNIX_EPOCH + Duration::from_secs(3600)).display_iso_8601().to_string(),
            "1970-01-01T01:00:00Z"
        );
    }

    #[test]
    fn display_out_of_range() {
        let time = SystemTime::from(Timestamp::MAX) + Duration::from_secs(12345);
        assert_eq!(time.display_iso_8601().to_string(), "9999-12-30T22:00:00.999999999Z");
    }

    #[test]
    fn after_unix_epoch_ok() {
        let now = SystemTime::now();
        assert!(after_unix_epoch(now));

        let past = SystemTime::UNIX_EPOCH - Duration::from_secs(12345);
        assert!(!after_unix_epoch(past));
    }
}
