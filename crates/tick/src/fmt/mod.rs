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
//! # Extension Traits
//!
//! - [`SystemTimeExt`]: Extension trait for [`SystemTime`][`std::time::SystemTime`] that provides convenient
//!   formatting methods. The [`display()`][SystemTimeExt::display] method returns an `impl Display` that
//!   formats the time in ISO 8601 format.
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
//! use tick::fmt::SystemTimeExt;
//!
//! let time = SystemTime::UNIX_EPOCH + Duration::from_secs(3600);
//! println!("Time: {}", time.display());
//! // Output: Time: 1970-01-01T01:00:00Z
//! ```

mod iso_8601;
mod rfc_2822;
mod system_time_ext;
mod unix_seconds;

pub use iso_8601::Iso8601;
pub use rfc_2822::Rfc2822;
pub use system_time_ext::SystemTimeExt;
pub use unix_seconds::UnixSeconds;

#[cfg(test)]
mod tests {
    use std::time::{Duration, SystemTime};

    use serde::{Deserialize, Serialize};

    use crate::Clock;

    use super::*;

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
        let clock = Clock::new_frozen_at(SystemTime::UNIX_EPOCH + Duration::from_millis(10_123_000));

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
    fn min_values_are_aligned() {
        // All MIN values should represent Unix epoch (1 January 1970 00:00:00 UTC)
        let iso_min: SystemTime = Iso8601::MIN.into();
        let rfc_min: SystemTime = Rfc2822::MIN.into();
        let unix_min: SystemTime = UnixSeconds::MIN.into();

        assert_eq!(iso_min, SystemTime::UNIX_EPOCH, "Iso8601::MIN should be Unix epoch");
        assert_eq!(rfc_min, SystemTime::UNIX_EPOCH, "Rfc2822::MIN should be Unix epoch");
        assert_eq!(unix_min, SystemTime::UNIX_EPOCH, "UnixSeconds::MIN should be Unix epoch");

        // Cross-format conversions at MIN should preserve the value
        assert_eq!(Iso8601::from(Rfc2822::MIN), Iso8601::MIN);
        assert_eq!(Iso8601::from(UnixSeconds::MIN), Iso8601::MIN);
        assert_eq!(Rfc2822::from(Iso8601::MIN), Rfc2822::MIN);
        assert_eq!(Rfc2822::from(UnixSeconds::MIN), Rfc2822::MIN);
        assert_eq!(UnixSeconds::from(Iso8601::MIN), UnixSeconds::MIN);
        assert_eq!(UnixSeconds::from(Rfc2822::MIN), UnixSeconds::MIN);
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
