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
//! which retrieves current system time and does the automatic conversion to the output format. This conversion never fails because clock
//! always returns a valid and normalized `SystemTime`.
//!
//! # Representable ranges and saturation
//!
//! Each format covers a different span of time:
//!
//! | Format | Minimum | Maximum |
//! | --- | --- | --- |
//! | [`Iso8601`] | [`Iso8601::MIN`] (`-009999-01-02T01:59:59Z`) | [`Iso8601::MAX`] (`9999-12-30T22:00:00.9999999Z`) |
//! | [`Rfc2822`] | [`Rfc2822::MIN`] (`Sat, 01 Jan 0000 00:00:00 GMT`) | [`Rfc2822::MAX`] (`Thu, 30 Dec 9999 22:00:00 GMT`) |
//! | [`UnixSeconds`] | [`UnixSeconds::MIN`] (`0`, the Unix epoch) | [`UnixSeconds::MAX`] (`253402207200`) |
//!
//! Converting an instant into a format that cannot represent it **saturates to the nearest
//! boundary of the target range** rather than panicking, wrapping, or mirroring the value.
//! This keeps every conversion between the formats infallible while never inventing an
//! instant on the other side of a boundary.
//!
//! ```
//! use tick::fmt::{Iso8601, Rfc2822, UnixSeconds};
//!
//! // `UnixSeconds` counts forward from the Unix epoch, so earlier instants saturate to it.
//! let before_epoch: Iso8601 = "1969-12-31T23:59:59Z".parse()?;
//! assert_eq!(UnixSeconds::from(before_epoch), UnixSeconds::MIN);
//!
//! // RFC 2822 encodes the year as four digits, so earlier instants saturate to year 0.
//! let before_year_zero: Iso8601 = "-000001-06-15T00:00:00Z".parse()?;
//! assert_eq!(Rfc2822::from(before_year_zero), Rfc2822::MIN);
//!
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! Converting any format back into a [`SystemTime`] saturates the same way, to the furthest
//! instant the platform can reach from the Unix epoch.
//!
//! # Examples
//!
//! ## Using format types
//!
//! ```
//! use tick::fmt::{Iso8601, Rfc2822, UnixSeconds};
//!
//! // ISO 8601
//! let time: Iso8601 = "2024-08-06T21:30:00Z".parse()?;
//! assert_eq!(time.to_string(), "2024-08-06T21:30:00Z");
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
//! ```

use std::time::{Duration, SystemTime};

use jiff::{SignedDuration, Timestamp};

mod iso_8601;
mod rfc_2822;
mod unix_seconds;

pub use iso_8601::Iso8601;
pub use rfc_2822::Rfc2822;
pub use unix_seconds::UnixSeconds;

/// Returns how long after the Unix epoch `timestamp` occurs.
///
/// Instants before the Unix epoch saturate to [`Duration::ZERO`], since the unsigned
/// [`Duration`] used by [`UnixSeconds`] cannot represent them.
fn to_unix_epoch_duration(timestamp: Timestamp) -> Duration {
    timestamp.as_duration().max(SignedDuration::ZERO).unsigned_abs()
}

/// Converts an offset from the Unix epoch into a [`SystemTime`].
///
/// Offsets the platform cannot apply saturate to the furthest [`SystemTime`] reachable from
/// the Unix epoch in that direction, so the conversion never panics.
fn to_system_time(offset: SignedDuration) -> SystemTime {
    saturating_offset(offset.is_negative(), offset.unsigned_abs())
}

/// Offsets [`SystemTime::UNIX_EPOCH`] by `magnitude`, saturating to the furthest instant the
/// platform can reach when the offset is too large to apply.
fn saturating_offset(negative: bool, magnitude: Duration) -> SystemTime {
    checked_offset(negative, magnitude).unwrap_or_else(|| saturating_bound(negative))
}

/// Offsets [`SystemTime::UNIX_EPOCH`] by `magnitude`, returning `None` when the platform
/// cannot apply an offset that large.
fn checked_offset(negative: bool, magnitude: Duration) -> Option<SystemTime> {
    if negative {
        SystemTime::UNIX_EPOCH.checked_sub(magnitude)
    } else {
        SystemTime::UNIX_EPOCH.checked_add(magnitude)
    }
}

/// Returns the [`SystemTime`] furthest from the Unix epoch the platform can reach in the
/// given direction.
fn saturating_bound(negative: bool) -> SystemTime {
    // How large an offset `SystemTime` accepts is platform defined and `std` does not expose
    // it, so the boundary is discovered here. This path is live: where `SystemTime` is
    // FILETIME-based its epoch is `1601-01-01T00:00:00Z`, so every earlier instant a
    // `jiff::Timestamp` can hold (down to year -9999) lands here.
    //
    // Representability shrinks monotonically as the offset grows, so the largest acceptable
    // offset is assembled one bit at a time from the most significant down: keep a bit
    // whenever the offset it produces still applies. A fixed trip count of `u64::BITS` makes
    // the walk incapable of looping, which a `lower`/`upper` binary search is not.
    let mut magnitude = 0_u64;

    for bit in (0..u64::BITS).rev() {
        // Addition rather than a bitwise set: this bit is always clear in `magnitude`, since
        // the walk visits each bit once and never revisits a higher one.
        let candidate = magnitude + (1 << bit);

        if checked_offset(negative, Duration::from_secs(candidate)).is_some() {
            magnitude = candidate;
        }
    }

    checked_offset(negative, Duration::from_secs(magnitude))
        .expect("the loop above only ever keeps a bit whose resulting offset it verified as applicable")
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use serde::{Deserialize, Serialize};

    use super::*;
    use crate::Clock;

    #[test]
    fn assert_json_format() {
        let clock = Clock::new_frozen_at(SystemTime::UNIX_EPOCH + Duration::from_millis(10_123_456));

        let dates = Dates {
            iso: clock.system_time_as::<Iso8601>(),
            rfc: clock.system_time_as::<Rfc2822>(),
            unix: clock.system_time_as::<UnixSeconds>(),
        };

        let json = serde_json::to_string(&dates).unwrap();

        assert_eq!(
            json,
            r#"{"iso":"1970-01-01T02:48:43.456Z","rfc":"Thu, 01 Jan 1970 02:48:43 GMT","unix":10123}"#
        );
    }

    #[test]
    fn assert_display_format() {
        let clock = Clock::new_frozen_at(SystemTime::UNIX_EPOCH + Duration::from_millis(10_123_456));

        let dates = Dates {
            iso: clock.system_time_as::<Iso8601>(),
            rfc: clock.system_time_as::<Rfc2822>(),
            unix: clock.system_time_as::<UnixSeconds>(),
        };

        let formatted = format!("iso: {}, unix: {}, rfc: {}", dates.iso, dates.unix, dates.rfc);
        assert_eq!(
            formatted,
            "iso: 1970-01-01T02:48:43.456Z, unix: 10123, rfc: Thu, 01 Jan 1970 02:48:43 GMT"
        );
    }

    #[test]
    fn json_roundtrip() {
        let clock = Clock::new_frozen_at(SystemTime::UNIX_EPOCH + Duration::from_secs(10123));

        let dates = Dates {
            iso: clock.system_time_as::<Iso8601>(),
            rfc: clock.system_time_as::<Rfc2822>(),
            unix: clock.system_time_as::<UnixSeconds>(),
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
    }

    #[test]
    fn unix_epoch_values_are_aligned() {
        // All UNIX_EPOCH values should represent Unix epoch (1 January 1970 00:00:00 UTC)
        let iso_epoch: SystemTime = Iso8601::UNIX_EPOCH.into();
        let rfc_epoch: SystemTime = Rfc2822::UNIX_EPOCH.into();
        let unix_epoch: SystemTime = UnixSeconds::UNIX_EPOCH.into();

        assert_eq!(iso_epoch, SystemTime::UNIX_EPOCH, "Iso8601::UNIX_EPOCH should be Unix epoch");
        assert_eq!(rfc_epoch, SystemTime::UNIX_EPOCH, "Rfc2822::UNIX_EPOCH should be Unix epoch");
        assert_eq!(unix_epoch, SystemTime::UNIX_EPOCH, "UnixSeconds::UNIX_EPOCH should be Unix epoch");

        // Cross-format conversions at UNIX_EPOCH should preserve the value
        assert_eq!(Iso8601::from(Rfc2822::UNIX_EPOCH), Iso8601::UNIX_EPOCH);
        assert_eq!(Iso8601::from(UnixSeconds::UNIX_EPOCH), Iso8601::UNIX_EPOCH);
        assert_eq!(Rfc2822::from(Iso8601::UNIX_EPOCH), Rfc2822::UNIX_EPOCH);
        assert_eq!(Rfc2822::from(UnixSeconds::UNIX_EPOCH), Rfc2822::UNIX_EPOCH);
        assert_eq!(UnixSeconds::from(Iso8601::UNIX_EPOCH), UnixSeconds::UNIX_EPOCH);
        assert_eq!(UnixSeconds::from(Rfc2822::UNIX_EPOCH), UnixSeconds::UNIX_EPOCH);
    }

    #[test]
    fn to_unix_epoch_duration_saturates_before_epoch() {
        let before_epoch = Timestamp::UNIX_EPOCH - SignedDuration::from_secs(1);

        // Without saturation this mirrors to one second *after* the epoch.
        assert_eq!(to_unix_epoch_duration(before_epoch), Duration::ZERO);
        assert_eq!(to_unix_epoch_duration(Timestamp::MIN), Duration::ZERO);
    }

    #[test]
    fn to_unix_epoch_duration_keeps_after_epoch() {
        assert_eq!(to_unix_epoch_duration(Timestamp::UNIX_EPOCH), Duration::ZERO);

        let after_epoch = Timestamp::UNIX_EPOCH + SignedDuration::from_secs(1);
        assert_eq!(to_unix_epoch_duration(after_epoch), Duration::from_secs(1));

        assert_eq!(to_unix_epoch_duration(Timestamp::MAX), UnixSeconds::MAX.0);
    }

    #[test]
    fn to_system_time_matches_epoch_offsets_in_range() {
        assert_eq!(to_system_time(SignedDuration::ZERO), SystemTime::UNIX_EPOCH);

        assert_eq!(
            to_system_time(SignedDuration::from_secs(90)),
            SystemTime::UNIX_EPOCH + Duration::from_secs(90)
        );
        assert_eq!(
            to_system_time(SignedDuration::from_secs(-90)),
            SystemTime::UNIX_EPOCH - Duration::from_secs(90)
        );

        // How far a platform's `SystemTime` reaches is not universal -- a FILETIME-based
        // `SystemTime` cannot go below 1601 -- so the extremes of the jiff range are checked
        // against the platform's own arithmetic rather than an assumed range.
        for timestamp in [Timestamp::MIN, Timestamp::MAX] {
            let offset = timestamp.as_duration();
            let negative = offset.is_negative();
            let converted = to_system_time(offset);

            let expected = if negative {
                SystemTime::UNIX_EPOCH.checked_sub(offset.unsigned_abs())
            } else {
                SystemTime::UNIX_EPOCH.checked_add(offset.unsigned_abs())
            };

            match expected {
                Some(expected) => assert_eq!(converted, expected, "{timestamp} must convert exactly"),
                None => assert_eq!(converted, saturating_bound(negative), "{timestamp} must saturate"),
            }
        }
    }

    #[test]
    fn saturating_offset_saturates_beyond_the_platform_bound() {
        for negative in [false, true] {
            // No platform can apply an offset of `Duration::MAX` to the Unix epoch, so this
            // always exercises the saturating fallback.
            let saturated = saturating_offset(negative, Duration::MAX);

            let magnitude = if negative {
                SystemTime::UNIX_EPOCH.duration_since(saturated).unwrap()
            } else {
                saturated.duration_since(SystemTime::UNIX_EPOCH).unwrap()
            };

            // Saturation lands exactly on the boundary: that offset applies, one more second
            // does not.
            assert_eq!(checked_offset(negative, magnitude), Some(saturated));
            assert_eq!(checked_offset(negative, magnitude + Duration::from_secs(1)), None);

            // Saturation is deterministic regardless of how far out of range the input is.
            assert_eq!(saturated, saturating_offset(negative, Duration::MAX - Duration::from_secs(1)));
        }
    }

    #[test]
    fn min_values_are_aligned() {
        // `UnixSeconds` cannot go below the epoch, and `Rfc2822` cannot go below year 0.
        assert_eq!(UnixSeconds::MIN, UnixSeconds::UNIX_EPOCH);
        assert!(Iso8601::MIN < Iso8601::from(Rfc2822::MIN));
        assert!(Iso8601::from(Rfc2822::MIN) < Iso8601::from(UnixSeconds::MIN));

        // Converting each format's minimum into a narrower format saturates to that minimum.
        assert_eq!(Rfc2822::from(Iso8601::MIN), Rfc2822::MIN);
        assert_eq!(UnixSeconds::from(Iso8601::MIN), UnixSeconds::MIN);
        assert_eq!(UnixSeconds::from(Rfc2822::MIN), UnixSeconds::MIN);

        // Widening conversions are lossless.
        assert_eq!(Iso8601::from(UnixSeconds::MIN), Iso8601::UNIX_EPOCH);
        assert_eq!(Rfc2822::from(UnixSeconds::MIN), Rfc2822::UNIX_EPOCH);
    }

    #[test]
    fn pre_epoch_to_unix_seconds_is_consistent_across_formats() {
        let iso: Iso8601 = "1900-01-01T00:00:00Z".parse().unwrap();
        let rfc: Rfc2822 = "Mon, 01 Jan 1900 00:00:00 GMT".parse().unwrap();

        assert_eq!(Iso8601::from(rfc), iso, "the two inputs must denote the same instant");
        assert_eq!(UnixSeconds::from(iso), UnixSeconds::MIN);
        assert_eq!(UnixSeconds::from(rfc), UnixSeconds::from(iso));
    }

    #[test]
    fn boundary_values_render_as_documented() {
        // Guards the representable-range table in this module's documentation. `Iso8601`
        // rounds nanoseconds down to 100 ns steps on display, so its `MAX` renders with
        // seven fractional digits rather than the nine `Timestamp::MAX` carries.
        assert_eq!(Iso8601::MIN.to_string(), "-009999-01-02T01:59:59Z");
        assert_eq!(Iso8601::MAX.to_string(), "9999-12-30T22:00:00.9999999Z");

        assert_eq!(Rfc2822::MIN.to_string(), "Sat, 01 Jan 0000 00:00:00 GMT");
        assert_eq!(Rfc2822::MAX.to_string(), "Thu, 30 Dec 9999 22:00:00 GMT");

        assert_eq!(UnixSeconds::MIN.to_string(), "0");
        assert_eq!(UnixSeconds::MAX.to_string(), "253402207200");
    }

    #[test]
    fn max_values_are_aligned() {
        // All MAX values should represent 31 December 9999 23:59:59 UTC
        let iso_max: SystemTime = Iso8601::MAX.into();
        let rfc_max: SystemTime = Rfc2822::MAX.into();
        let unix_max: SystemTime = UnixSeconds::MAX.into();

        assert_eq!(iso_max, rfc_max, "Iso8601::MAX and Rfc2822::MAX should be equal");
        assert_eq!(iso_max, unix_max, "Iso8601::MAX and UnixSeconds::MAX should be equal");

        // Cross-format conversions at MAX should preserve the value
        assert_eq!(Iso8601::from(Rfc2822::MAX), Iso8601::MAX);
        assert_eq!(Iso8601::from(UnixSeconds::MAX), Iso8601::MAX);
        assert_eq!(Rfc2822::from(Iso8601::MAX), Rfc2822::MAX);
        assert_eq!(Rfc2822::from(UnixSeconds::MAX), Rfc2822::MAX);
        assert_eq!(UnixSeconds::from(Iso8601::MAX), UnixSeconds::MAX);
        assert_eq!(UnixSeconds::from(Rfc2822::MAX), UnixSeconds::MAX);
    }
}
