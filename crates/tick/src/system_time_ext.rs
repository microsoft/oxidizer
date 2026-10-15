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

    /// Formats the [`SystemTime`] in the ECMAScript Date Time String Format.
    ///
    /// For years `0000` through `9999` the output has the fixed 24-character shape
    /// `YYYY-MM-DDTHH:MM:SS.sssZ`, truncated to millisecond precision. See
    /// [`EcmaScript`][crate::fmt::EcmaScript] for details.
    ///
    /// Times outside the supported timestamp range are saturated to the nearest
    /// boundary. The lower boundary has a negative year and renders in the wider
    /// ECMAScript expanded-year form rather than the fixed 24-character shape.
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

    use super::*;

    #[test]
    fn private_fallback_selects_the_nearest_jiff_boundary() {
        // The helper is only reachable after the public conversion has failed.
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(12345);
        assert_eq!(to_timestamp_min_max(now), jiff::Timestamp::MAX);

        let past = SystemTime::UNIX_EPOCH - Duration::from_secs(12345);
        assert_eq!(to_timestamp_min_max(past), jiff::Timestamp::MIN);
    }
}
