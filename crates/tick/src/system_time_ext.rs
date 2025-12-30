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
            match system_time.duration_since(SystemTime::UNIX_EPOCH) {
                Ok(_) => Timestamp::MAX,
                Err(_) => Timestamp::MIN, // earlier than UNIX_EPOCH, so this must be Timestamp::MIN
            }
        }
    }
}

mod sealed {
    pub trait Sealed {}

    impl Sealed for std::time::SystemTime {}
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn display_ok() {
        assert_eq!(SystemTime::UNIX_EPOCH.display_iso_8601().to_string(), "1970-01-01T00:00:00Z");

        assert_eq!(
            (SystemTime::UNIX_EPOCH + Duration::from_secs(3600)).display_iso_8601().to_string(),
            "1970-01-01T01:00:00Z"
        );

        // out of range
        assert_eq!(
            (SystemTime::UNIX_EPOCH + Duration::from_secs(3600 * 24 * 365 * 20000))
                .display_iso_8601()
                .to_string(),
            "9999-12-30T22:00:00.999999999Z"
        );

        assert_eq!(
            (SystemTime::UNIX_EPOCH - Duration::from_secs(3600 * 24 * 365 * 20000))
                .display_iso_8601()
                .to_string(),
            "-009999-01-02T01:59:59Z"
        );
    }
}
