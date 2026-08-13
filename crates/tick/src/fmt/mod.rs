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
//! To retrieve the current system time in the respective format, use the [`Clock::system_time_as`][crate::Clock::system_time_as] function
//! which retrieves current system time and does the automatic conversion to the output format. It panics when the target format cannot
//! represent the clock's instant, which a controlled clock can produce; use [`Clock::system_time`][crate::Clock::system_time] with the
//! target type's [`TryFrom`] where that has to be handled.
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
//! UnixSeconds::try_from(SystemTime::UNIX_EPOCH - std::time::Duration::from_secs(1)).unwrap_err();
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
//! UnixSeconds::try_from(SystemTime::from(before_epoch)).unwrap_err();
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

pub use ecmascript::EcmaScript;
pub use iso_8601::Iso8601;
pub use rfc_2822::Rfc2822;
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

    use ::serde::{Deserialize, Serialize};
    use jiff::SignedDuration;

    use super::*;
    use crate::Clock;

    #[test]
    fn assert_json_format() {
        let clock = Clock::new_frozen_at(SystemTime::UNIX_EPOCH + Duration::from_millis(10_123_456));

        let dates = Dates {
            iso: clock.system_time_as::<Iso8601>(),
            rfc: clock.system_time_as::<Rfc2822>(),
            unix: clock.system_time_as::<UnixSeconds>(),
            ecma: clock.system_time_as::<EcmaScript>(),
        };

        let json = serde_json::to_string(&dates).unwrap();

        assert_eq!(
            json,
            r#"{"iso":"1970-01-01T02:48:43.456Z","rfc":"Thu, 01 Jan 1970 02:48:43 GMT","unix":10123,"ecma":"1970-01-01T02:48:43.456Z"}"#
        );
    }

    #[test]
    fn assert_display_format() {
        let clock = Clock::new_frozen_at(SystemTime::UNIX_EPOCH + Duration::from_millis(10_123_456));

        let dates = Dates {
            iso: clock.system_time_as::<Iso8601>(),
            rfc: clock.system_time_as::<Rfc2822>(),
            unix: clock.system_time_as::<UnixSeconds>(),
            ecma: clock.system_time_as::<EcmaScript>(),
        };

        let formatted = format!("iso: {}, unix: {}, rfc: {}, ecma: {}", dates.iso, dates.unix, dates.rfc, dates.ecma);
        assert_eq!(
            formatted,
            "iso: 1970-01-01T02:48:43.456Z, unix: 10123, rfc: Thu, 01 Jan 1970 02:48:43 GMT, ecma: 1970-01-01T02:48:43.456Z"
        );
    }

    #[test]
    fn json_roundtrip() {
        let clock = Clock::new_frozen_at(SystemTime::UNIX_EPOCH + Duration::from_secs(10123));

        let dates = Dates {
            iso: clock.system_time_as::<Iso8601>(),
            rfc: clock.system_time_as::<Rfc2822>(),
            unix: clock.system_time_as::<UnixSeconds>(),
            ecma: clock.system_time_as::<EcmaScript>(),
        };

        let json = serde_json::to_string(&dates).unwrap();
        let parsed: Dates = serde_json::from_str(&json).unwrap();
        assert_eq!(dates, parsed);
    }

    #[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
    struct Dates {
        iso: Iso8601,
        rfc: Rfc2822,
        unix: UnixSeconds,
        ecma: EcmaScript,
    }

    #[test]
    fn unix_epoch_values_are_aligned() {
        // All UNIX_EPOCH values should represent Unix epoch (1 January 1970 00:00:00 UTC)
        let iso_epoch: SystemTime = Iso8601::UNIX_EPOCH.into();
        let rfc_epoch: SystemTime = Rfc2822::UNIX_EPOCH.into();
        let unix_epoch: SystemTime = UnixSeconds::UNIX_EPOCH.into();
        let ecma_epoch: SystemTime = EcmaScript::UNIX_EPOCH.into();

        assert_eq!(iso_epoch, SystemTime::UNIX_EPOCH, "Iso8601::UNIX_EPOCH should be Unix epoch");
        assert_eq!(rfc_epoch, SystemTime::UNIX_EPOCH, "Rfc2822::UNIX_EPOCH should be Unix epoch");
        assert_eq!(unix_epoch, SystemTime::UNIX_EPOCH, "UnixSeconds::UNIX_EPOCH should be Unix epoch");
        assert_eq!(ecma_epoch, SystemTime::UNIX_EPOCH, "EcmaScript::UNIX_EPOCH should be Unix epoch");
    }

    #[test]
    fn checked_system_time_reports_platform_range() {
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
    fn ensure_system_time_representable_passes_in_range() {
        assert_eq!(
            ensure_system_time_representable(Timestamp::UNIX_EPOCH).unwrap(),
            Timestamp::UNIX_EPOCH
        );
        assert_eq!(ensure_system_time_representable(Timestamp::MAX).unwrap(), Timestamp::MAX);
    }

    #[test]
    fn ensure_system_time_representable_rejects_out_of_range() {
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
    fn system_time_out_of_range_describes_the_platform_limit() {
        // `checked_system_time` can only fail where `SystemTime` is narrower than the jiff
        // range, so the error is built directly to keep it exercised on every platform.
        assert_eq!(
            system_time_out_of_range().to_string(),
            "the instant is outside the range that `SystemTime` can represent on this platform"
        );
    }

    #[test]
    fn parsing_agrees_with_system_time_range() {
        // Whatever the platform supports, parsing and conversion must agree: a value that
        // parses can always be converted back, and one that cannot be represented is rejected.
        for input in ["1000-01-01T00:00:00Z", "0001-01-01T00:00:00Z", "1601-01-01T00:00:00Z"] {
            let timestamp: Timestamp = input.parse().unwrap();
            let parsed_iso = input.parse::<Iso8601>();
            let parsed_ecma = input.parse::<EcmaScript>();

            if let Some(expected) = checked_system_time(timestamp) {
                assert_eq!(SystemTime::from(parsed_iso.unwrap()), expected, "{input}");
                assert_eq!(SystemTime::from(parsed_ecma.unwrap()), expected, "{input}");
            } else {
                parsed_iso.expect_err("the instant must be rejected by Iso8601");
                parsed_ecma.expect_err("the instant must be rejected by EcmaScript");
            }
        }
    }

    #[test]
    fn formats_do_not_convert_between_each_other() {
        // Each format has a different representable range, so conversions go through
        // `SystemTime` and the fallible step stays visible.
        static_assertions::assert_not_impl_any!(Iso8601: From<EcmaScript>, From<Rfc2822>, From<UnixSeconds>);
        static_assertions::assert_not_impl_any!(EcmaScript: From<Iso8601>, From<Rfc2822>, From<UnixSeconds>);
        static_assertions::assert_not_impl_any!(Rfc2822: From<EcmaScript>, From<Iso8601>, From<UnixSeconds>);
        static_assertions::assert_not_impl_any!(UnixSeconds: From<EcmaScript>, From<Iso8601>, From<Rfc2822>);
    }

    #[test]
    fn max_values_match_each_format_resolution() {
        let iso_max: SystemTime = Iso8601::MAX.into();
        let rfc_max: SystemTime = Rfc2822::MAX.into();
        let ecma_max: SystemTime = EcmaScript::MAX.into();
        let unix_max: SystemTime = UnixSeconds::MAX.into();

        assert_eq!(iso_max, rfc_max, "Iso8601::MAX and Rfc2822::MAX should be equal");
        // A platform may truncate all three values to a coarser SystemTime grid.
        assert!(unix_max <= ecma_max, "UnixSeconds::MAX must not exceed EcmaScript::MAX");
        assert!(ecma_max <= iso_max, "EcmaScript::MAX must not exceed Iso8601::MAX");

        assert_eq!(Iso8601::MAX.to_string(), "9999-12-30T22:00:00.9999999Z");
        assert_eq!(Rfc2822::MAX.to_string(), "Thu, 30 Dec 9999 22:00:00 GMT");
        assert_eq!(EcmaScript::MAX.to_string(), "9999-12-30T22:00:00.999Z");
        assert_eq!(UnixSeconds::MAX.to_string(), "253402207200");
    }
}
