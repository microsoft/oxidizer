// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Module that contains primitives for parsing, formatting, and serializing [`SystemTime`][`std::time::SystemTime`]
//! into various formats.
//!
//! The following formats are available:
//!
//! - [`Iso8601`]: Parsing and formatting of system time in [ISO 8601](https://en.wikipedia.org/wiki/ISO_8601) format.
//!   For example, `2024-08-06T21:30:00Z`.
//!
//! - [`EcmaScript`]: Parsing and formatting of system time in the
//!   [ECMAScript Date Time String Format](https://tc39.es/ecma262/#sec-date-time-string-format), the profile produced by
//!   the ECMAScript `Date.prototype.toISOString` method. For example, `2024-08-06T21:30:00.123Z`.
//!
//! - [`Rfc2822`]: Parsing and formatting of system time in [RFC 2822](https://tools.ietf.org/html/rfc2822#section-3.3) format.
//!   For example, `Tue, 6 Aug 2024 14:30:00 -0000`.
//!
//! - [`UnixSeconds`]: Parsing and formatting of system time that is represented as the number of whole seconds since Unix epoch.
//!   For example, `0` represents `Thu, 1 Jan 1970 00:00:00 -0000`.
//!
//! # Interoperability with `SystemTime`
//!
//! Types in this module use the [`TryFrom`] trait to convert from `SystemTime` to the respective format. The conversion is fallible
//! because the `SystemTime` can be outside the maximum range of the respective format. The conversion back to `SystemTime` is
//! always infallible.
//!
//! To retrieve the current system time in the respective format, use
//! [`Clock::try_system_time_as`][crate::Clock::try_system_time_as]. It retrieves the current
//! system time and performs the target type's [`TryFrom`] conversion.
//!
//! # Representable range
//!
//! Every value of these types is validated when it is created, whether by parsing or by
//! converting from a [`SystemTime`]. A value that the platform's [`SystemTime`] cannot
//! represent, or that the format itself cannot encode, is **rejected** rather than clamped
//! or wrapped to a different value.
//!
//! ```
//! use std::time::SystemTime;
//!
//! use tick::fmt::{Iso8601, UnixSeconds};
//!
//! // Whatever parses can always be converted back, on every platform.
//! let iso: Iso8601 = "2024-08-06T21:30:00Z".parse()?;
//! let system_time = SystemTime::from(iso);
//! assert_eq!(Iso8601::try_from(system_time)?, iso);
//!
//! // `UnixSeconds` counts forward from the Unix epoch, so an earlier instant is rejected.
//! let before_epoch = SystemTime::UNIX_EPOCH - std::time::Duration::from_secs(1);
//! assert!(UnixSeconds::try_from(before_epoch).is_err());
//!
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! How far back [`EcmaScript`], [`Iso8601`], and [`Rfc2822`] reach is platform dependent:
//! where [`SystemTime`] is FILETIME based its epoch is `1601-01-01T00:00:00Z`, and earlier
//! instants are rejected. It also bounds resolution, counting in 100-nanosecond intervals
//! there, so any conversion routed through it truncates a finer instant to that grid.
//!
//! Because the range is enforced up front, converting any of these types back into a
//! [`SystemTime`] is infallible and cannot panic.
//!
//! Converting directly between the formats is deliberately not supported: each has a
//! different representable range, so such a conversion would have to be fallible or lossy.
//! Go through [`SystemTime`] instead, which makes the fallible step explicit.
//!
//! ```
//! use std::time::SystemTime;
//!
//! use tick::fmt::{Iso8601, Rfc2822, UnixSeconds};
//!
//! // ISO 8601 to RFC 2822, with the range check in plain sight.
//! let iso: Iso8601 = "2024-08-06T21:30:00Z".parse()?;
//! let rfc = Rfc2822::try_from(SystemTime::from(iso))?;
//! assert_eq!(rfc.to_string(), "Tue, 06 Aug 2024 21:30:00 GMT");
//!
//! // And on to Unix seconds the same way.
//! let unix = UnixSeconds::try_from(SystemTime::from(rfc))?;
//! assert_eq!(unix.to_string(), "1722979800");
//!
//! // The fallible step is explicit: `UnixSeconds` cannot express a pre-epoch instant.
//! let before_epoch: Iso8601 = "1969-12-31T23:59:59Z".parse()?;
//! assert!(UnixSeconds::try_from(SystemTime::from(before_epoch)).is_err());
//!
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! # Examples
//!
//! ## Using format types
//!
//! ```
//! use tick::fmt::{EcmaScript, Iso8601, Rfc2822, UnixSeconds};
//!
//! // ISO 8601
//! let time: Iso8601 = "2024-08-06T21:30:00Z".parse()?;
//! assert_eq!(time.to_string(), "2024-08-06T21:30:00Z");
//!
//! // ECMAScript (fixed-width, milliseconds)
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
//! ## Using `SystemTimeExt`
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

#[doc(inline)]
pub use ecmascript::EcmaScript;
#[doc(inline)]
pub use iso_8601::Iso8601;
#[doc(inline)]
pub use rfc_2822::Rfc2822;
#[doc(inline)]
pub use unix_seconds::UnixSeconds;

use crate::Error;

/// Converts `timestamp` to `SystemTime` if the platform can represent it.
///
/// Returns `None` otherwise. The representable range of [`SystemTime`] is platform defined:
/// where it is FILETIME based, its epoch is `1601-01-01T00:00:00Z` and nothing earlier can be
/// expressed, so the platform is asked directly rather than assumed.
fn checked_system_time(timestamp: Timestamp) -> Option<SystemTime> {
    let offset = timestamp.as_duration();
    let magnitude = offset.unsigned_abs();

    if offset.is_negative() {
        SystemTime::UNIX_EPOCH.checked_sub(magnitude)
    } else {
        SystemTime::UNIX_EPOCH.checked_add(magnitude)
    }
}

/// Rejects `timestamp` unless the platform's [`SystemTime`] can represent it.
///
/// Applied wherever one of these formats is created, so that converting the result back into
/// a [`SystemTime`] is infallible.
///
/// # Errors
///
/// Returns an error if the instant is outside the range of [`SystemTime`].
fn ensure_system_time_representable(timestamp: Timestamp) -> Result<Timestamp, Error> {
    checked_system_time(timestamp)
        .map(|_| timestamp)
        .ok_or_else(system_time_out_of_range)
}

/// The error reported for an instant this platform's [`SystemTime`] cannot express.
///
/// Named separately so it stays reachable from tests. Whether [`checked_system_time`] can
/// fail at all is a property of the platform, so where `SystemTime` spans the whole jiff
/// range no input produces this error and the message would otherwise never be exercised.
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
            assert_eq!(
                error.unwrap_err().to_string(),
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
        assert_eq!(
            system_time_out_of_range().to_string(),
            "the instant is outside the range that `SystemTime` can represent on this platform"
        );
    }
}
