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
    ///
    /// use tick::SystemTimeExt;
    ///
    /// let time = SystemTime::UNIX_EPOCH + Duration::from_hours(1);
    /// assert_eq!(time.display_iso_8601().to_string(), "1970-01-01T01:00:00Z");
    /// ```
    #[cfg(any(feature = "fmt", test))]
    fn display_iso_8601(&self) -> impl std::fmt::Display;

    /// Returns a value that formats the [`SystemTime`] in the ECMAScript Date Time String Format.
    ///
    /// For any non-negative year the output has the fixed 24-character shape
    /// `YYYY-MM-DDTHH:MM:SS.sssZ`, truncated to millisecond precision. See
    /// [`EcmaScript`][crate::fmt::EcmaScript] for details.
    ///
    /// Times outside the valid range (before year -9999 or after year 9999) are saturated
    /// to the nearest boundary. Saturating below the minimum yields a negative year, which
    /// renders in the wider ECMAScript expanded-year form rather than the fixed 24-character shape.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::time::{Duration, SystemTime};
    ///
    /// use tick::SystemTimeExt;
    ///
    /// let time = SystemTime::UNIX_EPOCH + Duration::from_hours(1);
    /// assert_eq!(
    ///     time.display_ecmascript().to_string(),
    ///     "1970-01-01T01:00:00.000Z"
    /// );
    /// ```
    #[cfg(any(feature = "fmt", test))]
    fn display_ecmascript(&self) -> impl std::fmt::Display;
}

impl SystemTimeExt for SystemTime {
    #[cfg(any(feature = "fmt", test))]
    fn display_iso_8601(&self) -> impl std::fmt::Display {
        // jiff's Timestamp implements Display that outputs ISO 8601 format
        to_timestamp(*self)
    }

    #[cfg(any(feature = "fmt", test))]
    fn display_ecmascript(&self) -> impl std::fmt::Display {
        crate::fmt::EcmaScript::from_timestamp(to_timestamp(*self))
    }
}

#[cfg(any(feature = "fmt", test))]
fn to_timestamp(system_time: SystemTime) -> jiff::Timestamp {
    match jiff::Timestamp::try_from(system_time) {
        Ok(timestamp) => timestamp,
        Err(_) => to_timestamp_min_max(system_time),
    }
}

#[cfg(any(feature = "fmt", test))]
fn to_timestamp_min_max(system_time: SystemTime) -> jiff::Timestamp {
    if system_time.duration_since(SystemTime::UNIX_EPOCH).is_ok() {
        jiff::Timestamp::MAX
    } else {
        jiff::Timestamp::MIN
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

    use jiff::Timestamp;

    use super::*;

    #[test]
    fn display_ok() {
        assert_eq!(SystemTime::UNIX_EPOCH.display_iso_8601().to_string(), "1970-01-01T00:00:00Z");

        assert_eq!(
            (SystemTime::UNIX_EPOCH + Duration::from_hours(1)).display_iso_8601().to_string(),
            "1970-01-01T01:00:00Z"
        );
    }

    #[test]
    fn display_out_of_range() {
        let time = SystemTime::from(Timestamp::MAX) + Duration::from_secs(12345);
        assert_eq!(time.display_iso_8601().to_string(), "9999-12-30T22:00:00.999999999Z");
    }

    #[test]
    fn display_ecmascript_ok() {
        assert_eq!(SystemTime::UNIX_EPOCH.display_ecmascript().to_string(), "1970-01-01T00:00:00.000Z");

        assert_eq!(
            (SystemTime::UNIX_EPOCH + Duration::from_hours(1)).display_ecmascript().to_string(),
            "1970-01-01T01:00:00.000Z"
        );
    }

    #[test]
    fn display_ecmascript_before_epoch() {
        let before = SystemTime::UNIX_EPOCH - Duration::from_secs(1);
        assert_eq!(before.display_ecmascript().to_string(), "1969-12-31T23:59:59.000Z");
    }

    #[test]
    fn display_ecmascript_out_of_range() {
        let time = SystemTime::from(Timestamp::MAX) + Duration::from_secs(12345);
        assert_eq!(time.display_ecmascript().to_string(), "9999-12-30T22:00:00.999Z");
    }

    // On Windows a `SystemTime` cannot represent an instant before the FILETIME
    // epoch (1601-01-01), so an instant below `Timestamp::MIN` (year -9999) is
    // unconstructable there and this saturation path is only reachable elsewhere.
    #[cfg(not(windows))]
    #[test]
    fn display_ecmascript_saturates_below_min() {
        let time = SystemTime::UNIX_EPOCH - Duration::from_secs(400_000_000_000);
        assert_eq!(time.display_ecmascript().to_string(), "-009999-01-02T01:59:59.000Z");
    }

    #[test]
    fn to_timestamp_fallback_ok() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(12345);
        assert_eq!(to_timestamp_min_max(now), jiff::Timestamp::MAX);

        let past = SystemTime::UNIX_EPOCH - Duration::from_secs(12345);
        assert_eq!(to_timestamp_min_max(past), jiff::Timestamp::MIN);
    }
}
