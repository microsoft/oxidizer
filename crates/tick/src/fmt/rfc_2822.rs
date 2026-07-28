// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt::{self, Debug, Display, Formatter};
use std::str::FromStr;
use std::time::SystemTime;

use jiff::Timestamp;
use jiff::fmt::rfc2822;

use crate::Error;
use crate::fmt::{Iso8601, UnixSeconds, to_system_time};

static RFC2822_PARSER: rfc2822::DateTimeParser = rfc2822::DateTimeParser::new();
static RFC2822_PRINTER: rfc2822::DateTimePrinter = rfc2822::DateTimePrinter::new();

/// Parser and formatter for system time in RFC 2822 format, typically used in HTTP headers.
///
/// Examples:
///
/// - `Thu, 08 Aug 2024 11:45:00 GMT` (UTC)
/// - `Tue, 06 Aug 2024 14:30:00 -0700` (UTC offset)
/// - `Wed, 07 Aug 2024 09:15:00 +0100` (UTC offset)
///
/// The RFC 2822 format is defined in [RFC 2822](https://tools.ietf.org/html/rfc2822#section-3.3).
///
/// # UTC and time zones
///
/// While RFC 2822 can include a UTC offset, the resulting [`Rfc2822`] is always represented in the
/// UTC time zone with an offset of `GMT` (zero).
///
/// # Serialization and deserialization
///
/// `Rfc2822` implements the `Serialize` and `Deserialize` traits from the `serde` crate.
/// The system time is serialized as a string using RFC 2822 format.
///
/// The serialization support is available when `serde` feature is enabled.
///
/// # Leap seconds
///
/// If an RFC 2822 string contains a leap second, parsing will succeed and the leap second will be trimmed.
///
/// ```
/// use tick::fmt::Rfc2822;
///
/// let rfc = "Mon, 31 Dec 1990 23:59:60 GMT".parse::<Rfc2822>()?;
/// assert_eq!(rfc.to_string(), "Mon, 31 Dec 1990 23:59:59 GMT");
///
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
///
/// # Representable range
///
/// RFC 2822 encodes the year as four digits, so this type spans [`Rfc2822::MIN`]
/// (`Sat, 01 Jan 0000 00:00:00 GMT`) through [`Rfc2822::MAX`] (`Thu, 30 Dec 9999 22:00:00 GMT`).
/// Converting an earlier instant into `Rfc2822` saturates to [`Rfc2822::MIN`], so the type can
/// never hold an instant it is unable to format.
///
/// ```
/// use tick::fmt::{Iso8601, Rfc2822};
///
/// let before_year_zero: Iso8601 = "-000001-06-15T00:00:00Z".parse()?;
/// let rfc = Rfc2822::from(before_year_zero);
///
/// assert_eq!(rfc, Rfc2822::MIN);
/// assert_eq!(rfc.to_string(), "Sat, 01 Jan 0000 00:00:00 GMT");
///
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
///
/// # Examples
///
/// ## Formatting and parsing - UTC
/// ```
/// use std::time::SystemTime;
///
/// use tick::fmt::Rfc2822;
///
/// let rfc = "Tue, 06 Aug 2024 21:30:00 GMT".parse::<Rfc2822>()?;
/// assert_eq!(rfc.to_string(), "Tue, 06 Aug 2024 21:30:00 GMT");
///
/// let system_time: SystemTime = rfc.into();
///
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
///
/// ### Formatting and parsing - With UTC offset
///
/// This example demonstrates that the UTC offset is applied to the resulting [`Rfc2822`].
/// Note that when formatting the absolute time, the UTC offset is not included in the formatted string.
/// ```
/// use std::time::SystemTime;
///
/// use tick::fmt::Rfc2822;
///
/// let rfc  = "Tue, 06 Aug 2024 14:30:00 -0700".parse::<Rfc2822>()?;
/// assert_eq!(rfc.to_string(), "Tue, 06 Aug 2024 21:30:00 GMT"); // Note that the UTC offset is applied
///
/// let system_time: SystemTime = rfc.into();
///
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Rfc2822(pub(super) Timestamp);

crate::thread_aware_move!(Rfc2822);

impl Rfc2822 {
    /// The largest value that can be represented by `Rfc2822`.
    ///
    /// This represents a Unix system time at `31 December 9999 23:59:59 UTC`.
    pub const MAX: Self = Self(Timestamp::MAX);

    /// The smallest value that can be represented by `Rfc2822`.
    ///
    /// This represents a Unix system time of `1 January 0000 00:00:00 UTC`. RFC 2822 encodes
    /// the year as four digits, so it cannot express any earlier instant.
    pub const MIN: Self = Self(Timestamp::constant(-62_167_219_200, 0));

    /// The Unix epoch represented as `Rfc2822`.
    ///
    /// This represents a Unix system time of `1 January 1970 00:00:00 UTC`.
    pub const UNIX_EPOCH: Self = Self(Timestamp::UNIX_EPOCH);

    /// Creates an `Rfc2822` that is guaranteed to be within the representable range,
    /// saturating to [`Rfc2822::MIN`] for earlier instants.
    ///
    /// Every `Rfc2822` is built through this constructor so that the type can never hold an
    /// instant its [`Display`] implementation is unable to encode.
    fn new_saturating(timestamp: Timestamp) -> Self {
        // Only the lower bound needs clamping: `Rfc2822::MAX` is `Timestamp::MAX`, so no
        // `Timestamp` can exceed it. `max_is_jiff_timestamp_max` guards that assumption.
        Self(timestamp.max(Self::MIN.0))
    }
}

impl FromStr for Rfc2822 {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let timestamp = RFC2822_PARSER.parse_timestamp(s).map_err(Error::jiff)?;

        Ok(Self::new_saturating(timestamp))
    }
}

#[expect(
    clippy::map_err_ignore,
    reason = "std::fmt::Error does not contain any data, so we ignore the inner error"
)]
impl Display for Rfc2822 {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        RFC2822_PRINTER
            .print_timestamp_rfc9110(&self.0, jiff::fmt::StdFmtWrite(f))
            .map_err(|_| fmt::Error)
    }
}

impl From<Rfc2822> for SystemTime {
    fn from(value: Rfc2822) -> Self {
        to_system_time(value.0.as_duration())
    }
}

impl TryFrom<SystemTime> for Rfc2822 {
    type Error = Error;

    fn try_from(value: SystemTime) -> Result<Self, Self::Error> {
        let timestamp = Timestamp::try_from(value).map_err(Error::jiff)?;
        Ok(Self::new_saturating(timestamp))
    }
}

impl From<Iso8601> for Rfc2822 {
    fn from(value: Iso8601) -> Self {
        Self::new_saturating(value.0)
    }
}

impl From<UnixSeconds> for Rfc2822 {
    fn from(value: UnixSeconds) -> Self {
        Self::new_saturating(Timestamp::UNIX_EPOCH + value.0)
    }
}

#[cfg(any(feature = "serde", test))]
impl serde_core::Serialize for Rfc2822 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde_core::Serializer,
    {
        serializer.collect_str(self)
    }
}

#[cfg(any(feature = "serde", test))]
impl<'de> serde_core::Deserialize<'de> for Rfc2822 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde_core::Deserializer<'de>,
    {
        String::deserialize(deserializer)?
            .parse::<Self>()
            .map_err(serde_core::de::Error::custom)
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use std::hash::Hash;
    use std::time::Duration;

    use super::*;
    static_assertions::assert_impl_all!(Rfc2822: Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, TryFrom<SystemTime>, From<Iso8601>, FromStr);

    #[test]
    fn display_does_not_panic_for_negative_year() {
        // AB#7661499: `to_string` used to panic because `Display` returned an error.
        let iso: Iso8601 = "-000001-06-15T00:00:00Z".parse().unwrap();
        let rfc: Rfc2822 = iso.into();

        assert_eq!(rfc, Rfc2822::MIN);
        assert_eq!(rfc.to_string(), "Sat, 01 Jan 0000 00:00:00 GMT");
        assert_eq!(format!("{rfc}"), "Sat, 01 Jan 0000 00:00:00 GMT");
    }

    #[test]
    fn display_does_not_panic_for_iso_8601_min() {
        let rfc: Rfc2822 = Iso8601::MIN.into();

        assert_eq!(rfc, Rfc2822::MIN);
        assert_eq!(rfc.to_string(), "Sat, 01 Jan 0000 00:00:00 GMT");
    }

    #[test]
    #[cfg(feature = "serde")]
    fn serialize_does_not_panic_for_negative_year() {
        // Serialization goes through `collect_str`, which panics on a failing `Display`.
        let rfc: Rfc2822 = Iso8601::MIN.into();

        let serialized = serde_json::to_string(&rfc).unwrap();
        assert_eq!(serialized, r#""Sat, 01 Jan 0000 00:00:00 GMT""#);

        let deserialized: Rfc2822 = serde_json::from_str(&serialized).unwrap();
        assert_eq!(deserialized, rfc);
    }

    #[test]
    fn min_is_earliest_encodable_instant() {
        assert_eq!(Rfc2822::MIN.to_string(), "Sat, 01 Jan 0000 00:00:00 GMT");
        assert!(Rfc2822::MIN < Rfc2822::UNIX_EPOCH);

        // One second earlier is not encodable, so it clamps back up to `MIN`.
        let one_second_earlier = Iso8601::from(Rfc2822::MIN);
        let one_second_earlier: SystemTime = one_second_earlier.into();
        let one_second_earlier = one_second_earlier - Duration::from_secs(1);

        assert_eq!(Rfc2822::try_from(one_second_earlier).unwrap(), Rfc2822::MIN);

        // The instant itself is preserved, not rounded away.
        assert_eq!(Rfc2822::try_from(SystemTime::from(Rfc2822::MIN)).unwrap(), Rfc2822::MIN);
    }

    #[test]
    fn max_is_jiff_timestamp_max() {
        // `new_saturating` only clamps the lower bound, which is sound only while no
        // `Timestamp` can exceed `Rfc2822::MAX`.
        assert_eq!(Rfc2822::MAX.0, Timestamp::MAX);
    }

    #[test]
    fn from_system_time_saturates_below_min() {
        let before_min = SystemTime::from(Iso8601::MIN);
        assert!(before_min < SystemTime::from(Rfc2822::MIN));

        assert_eq!(Rfc2822::try_from(before_min).unwrap(), Rfc2822::MIN);
    }

    #[test]
    fn to_unix_seconds_saturates_before_epoch() {
        // AB#7661495 for the RFC 2822 source format.
        let rfc: Rfc2822 = "Wed, 31 Dec 1969 23:59:59 GMT".parse().unwrap();
        assert_eq!(UnixSeconds::from(rfc), UnixSeconds::MIN);

        assert_eq!(UnixSeconds::from(Rfc2822::MIN), UnixSeconds::MIN);

        // Post-epoch values are unaffected.
        let rfc: Rfc2822 = "Thu, 01 Jan 1970 00:00:01 GMT".parse().unwrap();
        assert_eq!(UnixSeconds::from(rfc).to_secs(), 1);
    }

    #[test]
    fn to_system_time_before_filetime_epoch() {
        // AB#7663342 for the RFC 2822 source format.
        let rfc: Rfc2822 = "Wed, 01 Jan 1000 00:00:00 GMT".parse().unwrap();
        let system_time = SystemTime::from(rfc);

        assert!(system_time < SystemTime::UNIX_EPOCH);
        assert_eq!(Rfc2822::try_from(system_time).unwrap(), rfc);

        assert_eq!(Rfc2822::try_from(SystemTime::from(Rfc2822::MIN)).unwrap(), Rfc2822::MIN);
    }

    #[test]
    fn parse_err() {
        "date".parse::<Rfc2822>().unwrap_err();
    }

    #[test]
    fn parse_min() {
        let stamp: Rfc2822 = "Thu, 1 Jan 1970 00:00:00 GMT".parse().unwrap();
        assert_eq!(SystemTime::from(stamp), SystemTime::UNIX_EPOCH);
    }

    #[test]
    fn to_system_time() {
        let stamp: Rfc2822 = "Thu, 1 Jan 1970 00:00:01 GMT".parse().unwrap();
        assert_eq!(SystemTime::from(stamp), SystemTime::UNIX_EPOCH + Duration::from_secs(1));
    }

    #[test]
    fn to_system_time_alternative_format() {
        let stamp: Rfc2822 = "Thu, 1 Jan 1970 00:00:01 -0000".parse().unwrap();
        assert_eq!(SystemTime::from(stamp), SystemTime::UNIX_EPOCH + Duration::from_secs(1));
    }

    #[test]
    fn parse_then_display() {
        let stamp: Rfc2822 = "Thu, 01 Jan 1970 01:00:00 GMT".parse().unwrap();

        // Display should return the timestamp in the RFC 2822 format
        assert_eq!(stamp.to_string(), "Thu, 01 Jan 1970 01:00:00 GMT");
        assert_eq!(SystemTime::from(stamp), SystemTime::UNIX_EPOCH + Duration::from_hours(1));
    }

    #[test]
    fn parse_display_leap_year() {
        let stamp: Rfc2822 = "Tue, 29 Feb 2000 01:00:00 GMT".parse().unwrap();
        assert_eq!(stamp.to_string(), "Tue, 29 Feb 2000 01:00:00 GMT");

        let secs = SystemTime::from(stamp).duration_since(SystemTime::UNIX_EPOCH).unwrap().as_secs();
        assert_eq!(secs, 951_786_000);
    }

    #[test]
    fn parse_max() {
        let stamp: Rfc2822 = "Thu, 30 Dec 9999 22:00:00 GMT".parse().unwrap();
        assert_eq!(stamp.to_string(), "Thu, 30 Dec 9999 22:00:00 GMT");
    }

    #[test]
    fn parse_max_overflow() {
        "Thu, 30 Dec 10000 22:00:00 GMT".parse::<Rfc2822>().unwrap_err();
    }

    #[test]
    fn parse_leap_seconds() {
        let stamp: Rfc2822 = "Mon, 31 Dec 1990 23:59:60 GMT".parse().unwrap();
        assert_eq!(stamp.to_string(), "Mon, 31 Dec 1990 23:59:59 GMT");
    }

    #[test]
    #[cfg(feature = "serde")]
    fn serialize_deserialize() {
        let iso: Rfc2822 = "Thu, 1 Jan 1970 01:00:00 GMT".parse().unwrap();
        let serialized = serde_json::to_string(&iso).unwrap();
        let deserialized: Rfc2822 = serde_json::from_str(&serialized).unwrap();

        assert_eq!(iso, deserialized);
    }
}
