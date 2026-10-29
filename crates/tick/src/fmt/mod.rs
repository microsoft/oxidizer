// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Parses, formats, and serializes [`SystemTime`][`std::time::SystemTime`].
//!
//! The module provides these formats:
//!
//! - [`Iso8601`] uses [ISO 8601](https://en.wikipedia.org/wiki/ISO_8601).
//!   Example: `2024-08-06T21:30:00Z`.
//! - [`EcmaScript`] uses the
//!   [ECMAScript Date Time String Format](https://tc39.es/ecma262/#sec-date-time-string-format).
//!   It matches `Date.prototype.toISOString`. Example:
//!   `2024-08-06T21:30:00.123Z`.
//! - [`Rfc2822`] uses [RFC 2822](https://tools.ietf.org/html/rfc2822#section-3.3).
//!   Example: `Tue, 6 Aug 2024 14:30:00 -0000`.
//! - [`UnixSeconds`] uses whole seconds since the Unix epoch. For example, `0`
//!   represents `Thu, 1 Jan 1970 00:00:00 -0000`.
//!
//! # Interoperability with `SystemTime`
//!
//! Each format implements [`TryFrom<SystemTime>`][TryFrom]. This conversion can
//! fail because the format can have a smaller range than `SystemTime`.
//! Conversion from a format back to `SystemTime` cannot fail.
//!
//! Use [`Clock::system_time_as`][crate::Clock::system_time_as] to get the
//! current time in one of these formats. This convenience method is
//! intentionally infallible for supported `tick` formats and production data.
//! Production data is a current operating-system clock value without
//! test-controlled offsets.
//!
//! If conversion failure is expected, call
//! [`Clock::system_time`][crate::Clock::system_time]. Then call the target
//! type's [`TryFrom`] implementation.
//!
//! # Representable range
//!
//! Each value is validated when it is created. This applies to parsed values
//! and values converted from [`SystemTime`]. Creation fails if the platform
//! cannot represent the value or the format cannot encode it. The value is not
//! clamped or wrapped.
//!
//! ```
//! use std::time::SystemTime;
//!
//! use tick::fmt::{Iso8601, UnixSeconds};
//!
//! // A parsed value can always convert back on the same platform.
//! let iso: Iso8601 = "2024-08-06T21:30:00Z".parse()?;
//! let system_time = SystemTime::from(iso);
//! assert_eq!(Iso8601::try_from(system_time)?, iso);
//!
//! // `UnixSeconds` rejects times before the Unix epoch.
//! let before_epoch = SystemTime::UNIX_EPOCH - std::time::Duration::from_secs(1);
//! assert!(UnixSeconds::try_from(before_epoch).is_err());
//!
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! The platform controls the earliest value for [`EcmaScript`], [`Iso8601`],
//! and [`Rfc2822`]. On a FILETIME platform, [`SystemTime`] starts at
//! `1601-01-01T00:00:00Z`. The types reject earlier times.
//!
//! FILETIME uses 100-nanosecond intervals. A conversion through FILETIME
//! truncates a more precise value to that interval.
//!
//! Range checks occur when a value is created. Conversion back to [`SystemTime`]
//! therefore cannot fail or panic.
//!
//! The types do not convert directly between formats. Each format has a
//! different range, so direct conversion can fail or lose data. Convert
//! through [`SystemTime`] to make the fallible step visible.
//!
//! ```
//! use std::time::SystemTime;
//!
//! use tick::fmt::{Iso8601, Rfc2822, UnixSeconds};
//!
//! // Convert ISO 8601 to RFC 2822.
//! let iso: Iso8601 = "2024-08-06T21:30:00Z".parse()?;
//! let rfc = Rfc2822::try_from(SystemTime::from(iso))?;
//! assert_eq!(rfc.to_string(), "Tue, 06 Aug 2024 21:30:00 GMT");
//!
//! // Convert RFC 2822 to Unix seconds.
//! let unix = UnixSeconds::try_from(SystemTime::from(rfc))?;
//! assert_eq!(unix.to_string(), "1722979800");
//!
//! // `UnixSeconds` cannot represent a time before the Unix epoch.
//! let before_epoch: Iso8601 = "1969-12-31T23:59:59Z".parse()?;
//! assert!(UnixSeconds::try_from(SystemTime::from(before_epoch)).is_err());
//!
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! # Examples
//!
//! ## Use format types
//!
//! ```
//! use tick::fmt::{EcmaScript, Iso8601, Rfc2822, UnixSeconds};
//!
//! // ISO 8601
//! let time: Iso8601 = "2024-08-06T21:30:00Z".parse()?;
//! assert_eq!(time.to_string(), "2024-08-06T21:30:00Z");
//!
//! // ECMAScript with fixed-width milliseconds.
//! let time: EcmaScript = "2024-08-06T21:30:00Z".parse()?;
//! assert_eq!(time.to_string(), "2024-08-06T21:30:00.000Z");
//!
//! // RFC 2822
//! let time: Rfc2822 = "Tue, 06 Aug 2024 14:30:00 GMT".parse()?;
//! assert_eq!(time.to_string(), "Tue, 06 Aug 2024 14:30:00 GMT");
//!
//! // Unix seconds
//! let time: UnixSeconds = "951786000".parse()?;
//! assert_eq!(time.to_string(), "951786000");
//!
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! ## Use `SystemTimeExt`
//!
//! ```
//! use std::time::{Duration, SystemTime};
//!
//! use tick::SystemTimeExt;
//!
//! let time = SystemTime::UNIX_EPOCH + Duration::from_hours(1);
//! println!("Time: {}", time.display_iso_8601());
//! // Output: Time: 1970-01-01T01:00:00Z
//!
//! println!("Time: {}", time.display_ecmascript());
//! // Output: Time: 1970-01-01T01:00:00.000Z
//! ```

use std::time::SystemTime;

use jiff::Timestamp;

mod ecmascript;
mod iso_8601;
mod rfc_2822;
#[cfg(any(feature = "serde", test))]
mod serde;
mod unix_seconds;

pub use ecmascript::EcmaScript;
pub use iso_8601::Iso8601;
pub use rfc_2822::Rfc2822;
pub use unix_seconds::UnixSeconds;

use crate::Error;

/// Converts `timestamp` to `SystemTime` when the platform can represent it.
///
/// Returns `None` otherwise. The platform defines the range of [`SystemTime`].
/// For example, FILETIME starts at `1601-01-01T00:00:00Z`. This function uses
/// platform arithmetic instead of assuming a range.
fn checked_system_time(timestamp: Timestamp) -> Option<SystemTime> {
    let offset = timestamp.as_duration();
    let magnitude = offset.unsigned_abs();

    if offset.is_negative() {
        SystemTime::UNIX_EPOCH.checked_sub(magnitude)
    } else {
        SystemTime::UNIX_EPOCH.checked_add(magnitude)
    }
}

/// Returns `timestamp` if the platform can represent it as [`SystemTime`].
///
/// Each format calls this function during creation. Conversion back to
/// [`SystemTime`] can then be infallible.
///
/// # Errors
///
/// Returns an error if the instant is outside the range of [`SystemTime`].
fn ensure_system_time_representable(timestamp: Timestamp) -> Result<Timestamp, Error> {
    checked_system_time(timestamp)
        .map(|_| timestamp)
        .ok_or_else(system_time_out_of_range)
}

/// Creates the error for a time outside the platform [`SystemTime`] range.
///
/// This separate function keeps the message testable. On platforms where
/// `SystemTime` covers the full jiff range, [`checked_system_time`] cannot fail.
fn system_time_out_of_range() -> Error {
    Error::out_of_range("the instant is outside the range that `SystemTime` can represent on this platform")
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use std::time::Duration;

    use jiff::SignedDuration;

    use super::*;

    #[test]
    fn private_system_time_range_check_matches_platform_arithmetic() {
        // The helper is private because public constructors apply this check automatically.
        assert_eq!(checked_system_time(Timestamp::UNIX_EPOCH), Some(SystemTime::UNIX_EPOCH));

        let after_epoch = Timestamp::UNIX_EPOCH + SignedDuration::from_secs(90);
        assert_eq!(
            checked_system_time(after_epoch),
            Some(SystemTime::UNIX_EPOCH + Duration::from_secs(90))
        );

        let before_epoch = Timestamp::UNIX_EPOCH - SignedDuration::from_secs(90);
        assert_eq!(
            checked_system_time(before_epoch),
            Some(SystemTime::UNIX_EPOCH - Duration::from_secs(90))
        );

        // How far `SystemTime` reaches is platform defined -- where it is FILETIME based it
        // stops at 1601 -- so the extremes are checked against the platform's own arithmetic.
        for timestamp in [Timestamp::MIN, Timestamp::MAX] {
            let offset = timestamp.as_duration();
            let expected = if offset.is_negative() {
                SystemTime::UNIX_EPOCH.checked_sub(offset.unsigned_abs())
            } else {
                SystemTime::UNIX_EPOCH.checked_add(offset.unsigned_abs())
            };

            assert_eq!(checked_system_time(timestamp), expected, "{timestamp}");
        }
    }

    #[test]
    fn private_system_time_range_guard_accepts_representable_values() {
        assert_eq!(
            ensure_system_time_representable(Timestamp::UNIX_EPOCH).unwrap(),
            Timestamp::UNIX_EPOCH
        );
        assert_eq!(ensure_system_time_representable(Timestamp::MAX).unwrap(), Timestamp::MAX);
    }

    #[test]
    fn private_system_time_range_guard_rejects_platform_out_of_range_values() {
        // Only platforms with a narrower `SystemTime` than the jiff range can observe the
        // rejection, so the assertion follows whatever this platform actually supports.
        let error = ensure_system_time_representable(Timestamp::MIN);

        if checked_system_time(Timestamp::MIN).is_none() {
            let error = error.unwrap_err().with_disabled_backtrace();
            assert_eq!(
                error.to_string(),
                "the instant is outside the range that `SystemTime` can represent on this platform"
            );
        } else {
            assert_eq!(error.unwrap(), Timestamp::MIN);
        }
    }

    #[test]
    fn private_system_time_range_error_describes_the_platform_limit() {
        // `checked_system_time` can only fail where `SystemTime` is narrower than the jiff
        // range, so the error is built directly to keep it exercised on every platform.
        let error = system_time_out_of_range().with_disabled_backtrace();
        assert_eq!(
            error.to_string(),
            "the instant is outside the range that `SystemTime` can represent on this platform"
        );
    }
}
