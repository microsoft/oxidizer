// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt::Display;
use std::time::SystemTime;

use jiff::Timestamp;

/// Extension trait for [`SystemTime`] that provides formatting capabilities.
pub trait SystemTimeExt {
    /// Returns a value that formats the [`SystemTime`] in ISO 8601 format.
    ///
    /// Times outside the valid range (before year -9999 or after year 9999) are saturated
    /// to the nearest boundary.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::time::{Duration, SystemTime};
    /// use tick::fmt::SystemTimeExt;
    ///
    /// let time = SystemTime::UNIX_EPOCH + Duration::from_secs(3600);
    /// assert_eq!(time.display().to_string(), "1970-01-01T01:00:00Z");
    /// ```
    fn display(&self) -> impl Display;
}

impl SystemTimeExt for SystemTime {
    fn display(&self) -> impl Display {
        // jiff's Timestamp implements Display that outputs ISO 8601 format
        to_timestamp_saturating(*self)
    }
}

fn to_timestamp_saturating(system_time: SystemTime) -> Timestamp {
    match Timestamp::try_from(system_time) {
        Ok(timestamp) => timestamp,
        Err(_) => {
            if system_time < SystemTime::from(Timestamp::MIN) {
                Timestamp::MIN
            } else {
                Timestamp::MAX
            }
        }
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn display_ok() {
        assert_eq!(SystemTime::UNIX_EPOCH.display().to_string(), "1970-01-01T00:00:00Z");

        assert_eq!(
            (SystemTime::UNIX_EPOCH + Duration::from_secs(3600)).display().to_string(),
            "1970-01-01T01:00:00Z"
        );

        // out of range
        assert_eq!(
            (SystemTime::UNIX_EPOCH + Duration::from_secs(3600 * 24 * 365 * 20000))
                .display()
                .to_string(),
            "9999-12-30T22:00:00.999999999Z"
        );

        assert_eq!(
            (SystemTime::UNIX_EPOCH - Duration::from_secs(3600 * 24 * 365 * 20000))
                .display()
                .to_string(),
            "-009999-01-02T01:59:59Z"
        );
    }
}
