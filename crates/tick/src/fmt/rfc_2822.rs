// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt::{self, Debug, Display, Formatter};
use std::str::FromStr;
use std::sync::LazyLock;
use std::time::SystemTime;

use jiff::Timestamp;
use jiff::fmt::rfc2822;

use crate::Error;
use crate::fmt::ensure_system_time_representable;

static RFC2822_PARSER: rfc2822::DateTimeParser = rfc2822::DateTimeParser::new();
static RFC2822_PRINTER: rfc2822::DateTimePrinter = rfc2822::DateTimePrinter::new();

/// The earliest instant RFC 2822 can encode, as this crate writes it.
const MIN_ENCODABLE: &str = "Sat, 01 Jan 0000 00:00:00 GMT";

/// [`MIN_ENCODABLE`] as a [`Timestamp`].
///
/// RFC 2822 writes the year as four digits, so nothing before year 0 can be formatted.
static MIN_TIMESTAMP: LazyLock<Timestamp> = LazyLock::new(|| {
    RFC2822_PARSER
        .parse_timestamp(MIN_ENCODABLE)
        .expect("`MIN_ENCODABLE` is a literal in exactly the format `RFC2822_PARSER` accepts")
});

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
/// Output follows the [IMF-fixdate](https://datatracker.ietf.org/doc/html/rfc9110#section-5.6.7)
/// profile RFC 9110 defines for HTTP dates; parsing accepts the broader RFC 2822 grammar.
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
/// RFC 2822 encodes the year as four digits, so this type cannot express an instant before
/// `Sat, 01 Jan 0000 00:00:00 GMT`, and its upper bound is [`Rfc2822::MAX`]
/// (`Thu, 30 Dec 9999 22:00:00 GMT`). Instants outside that range, or outside what the
/// platform's [`SystemTime`] can represent, are rejected when the value is created, so the
/// type can never hold something it is unable to format.
///
/// ```
/// use tick::fmt::Rfc2822;
///
/// // This string is ISO 8601, not RFC 2822, so the parser rejects it as malformed; RFC 2822
/// // cannot write a year outside `0000` through `9999` at all. The year bound itself is
/// // enforced on the `TryFrom<SystemTime>` path, where such an instant can arrive.
/// "-0001-06-15T00:00:00Z".parse::<Rfc2822>().unwrap_err();
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
pub struct Rfc2822(Timestamp);

crate::thread_aware_move!(Rfc2822);

impl Rfc2822 {
    /// The largest value that can be represented by `Rfc2822`.
    ///
    /// This represents a Unix system time of `Thu, 30 Dec 9999 22:00:00 GMT`.
    pub const MAX: Self = Self(Timestamp::MAX);

    /// The Unix epoch represented as `Rfc2822`.
    ///
    /// This represents a Unix system time of `1 January 1970 00:00:00 UTC`.
    pub const UNIX_EPOCH: Self = Self(Timestamp::UNIX_EPOCH);

    /// Rejects `timestamp` unless it can be both formatted as RFC 2822 and represented as a
    /// [`SystemTime`].
    ///
    /// Every way to create an `Rfc2822` goes through here, so the type can never hold an
    /// instant its [`Display`] implementation is unable to encode.
    ///
    /// # Errors
    ///
    /// Returns an error if the instant is outside either range.
    fn new_checked(timestamp: Timestamp) -> Result<Self, Error> {
        // Only the lower bound needs checking: `Rfc2822::MAX` is `Timestamp::MAX`, so no
        // `Timestamp` can exceed it. Asserted here as well as in
        // `max_is_the_largest_supported_instant`, so a jiff upgrade that breaks the
        // assumption fails wherever it is relied on rather than only in that one test.
        debug_assert!(timestamp <= Self::MAX.0, "`Rfc2822::MAX` must stay the largest `Timestamp`");

        if timestamp < *MIN_TIMESTAMP {
            return Err(Error::out_of_range(
                "the instant is before year 0 and cannot be encoded as an RFC 2822 four-digit year",
            ));
        }

        Ok(Self(ensure_system_time_representable(timestamp)?))
    }
}

impl FromStr for Rfc2822 {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let timestamp = RFC2822_PARSER.parse_timestamp(s).map_err(Error::jiff)?;

        Self::new_checked(timestamp)
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
        // jiff's conversion panics for an instant this platform's `SystemTime` cannot hold,
        // which no `Rfc2822` can be: every constructor routes through `new_checked`, and the
        // constants are within range everywhere.
        value.0.into()
    }
}

impl TryFrom<SystemTime> for Rfc2822 {
    type Error = Error;

    fn try_from(value: SystemTime) -> Result<Self, Self::Error> {
        let timestamp = Timestamp::try_from(value).map_err(Error::jiff)?;
        Self::new_checked(timestamp)
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
        super::serde::deserialize_from_str(deserializer)
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use std::hash::Hash;
    use std::time::Duration;

    use jiff::SignedDuration;

    use super::*;
    static_assertions::assert_impl_all!(Rfc2822: Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, TryFrom<SystemTime>, FromStr);

    #[test]
    fn display_cannot_fail_for_any_reachable_value() {
        // Public construction rejects every instant the formatter cannot encode: parsing
        // yields a four-digit year, and `TryFrom<SystemTime>` rejects anything before year 0.
        // AB#7661499.
        for input in ["Thu, 01 Jan 1970 00:00:00 GMT", "Tue, 06 Aug 2024 21:30:00 GMT"] {
            let rfc: Rfc2822 = input.parse().unwrap();

            assert_eq!(rfc.to_string(), input);
            assert_eq!(format!("{rfc}"), input);
        }

        // Both ends of what the format itself can encode. `MIN_TIMESTAMP` is constructed
        // directly because platforms whose `SystemTime` starts at 1601 reject it.
        assert_eq!(Rfc2822(*MIN_TIMESTAMP).to_string(), "Sat, 01 Jan 0000 00:00:00 GMT");
        assert_eq!(Rfc2822::MAX.to_string(), "Thu, 30 Dec 9999 22:00:00 GMT");
    }

    #[test]
    #[cfg(feature = "serde")]
    fn serialize_cannot_fail_at_the_lower_bound() {
        // Serialization goes through `collect_str`, which panics on a failing `Display`.
        let rfc = Rfc2822(*MIN_TIMESTAMP);

        let serialized = serde_json::to_string(&rfc).unwrap();
        assert_eq!(serialized, r#""Sat, 01 Jan 0000 00:00:00 GMT""#);
    }

    #[test]
    fn min_timestamp_is_the_earliest_encodable_instant() {
        let min = Rfc2822(*MIN_TIMESTAMP);

        assert_eq!(min.to_string(), "Sat, 01 Jan 0000 00:00:00 GMT");
        assert!(min < Rfc2822::UNIX_EPOCH);

        // Anything earlier cannot be encoded, so `new_checked` rejects it. The RFC 2822 bound
        // is checked before the platform one, so this error is the same everywhere.
        for earlier in [SignedDuration::from_secs(1), SignedDuration::from_nanos(1)] {
            let error = Rfc2822::new_checked(*MIN_TIMESTAMP - earlier).unwrap_err();

            assert_eq!(
                error.to_string(),
                "the instant is before year 0 and cannot be encoded as an RFC 2822 four-digit year"
            );
        }

        // The instant itself clears the RFC 2822 bound, so it is accepted wherever the
        // platform's `SystemTime` reaches that far back.
        if crate::fmt::checked_system_time(*MIN_TIMESTAMP).is_some() {
            assert_eq!(Rfc2822::new_checked(*MIN_TIMESTAMP).unwrap(), min);
        } else {
            assert_eq!(
                Rfc2822::new_checked(*MIN_TIMESTAMP).unwrap_err().to_string(),
                "the instant is outside the range that `SystemTime` can represent on this platform"
            );
        }
    }

    #[test]
    fn max_is_the_largest_supported_instant() {
        // `new_checked` only guards the lower bound, which is sound only while no
        // `Timestamp` can exceed `Rfc2822::MAX`.
        assert_eq!(Rfc2822::MAX.0, Timestamp::MAX);
    }

    #[test]
    fn from_system_time_rejects_unencodable_years() {
        // Where the platform can express an instant before year 0, it must be rejected
        // rather than silently moved into range.
        let below_min = MIN_TIMESTAMP.as_duration().unsigned_abs() + Duration::from_secs(1);

        if let Some(below_min) = SystemTime::UNIX_EPOCH.checked_sub(below_min) {
            assert_eq!(
                Rfc2822::try_from(below_min).unwrap_err().to_string(),
                "the instant is before year 0 and cannot be encoded as an RFC 2822 four-digit year"
            );
        }

        // In-range instants are accepted and round-trip exactly.
        let rfc = Rfc2822::try_from(SystemTime::UNIX_EPOCH).unwrap();
        assert_eq!(rfc, Rfc2822::UNIX_EPOCH);
        assert_eq!(SystemTime::from(rfc), SystemTime::UNIX_EPOCH);
    }

    #[test]
    fn parse_rejects_instants_system_time_cannot_represent() {
        // AB#7663342 for the RFC 2822 source format.
        let input = "Wed, 01 Jan 1000 00:00:00 GMT";
        let timestamp = RFC2822_PARSER.parse_timestamp(input).unwrap();
        let parsed = input.parse::<Rfc2822>();

        if let Some(expected) = crate::fmt::checked_system_time(timestamp) {
            assert_eq!(SystemTime::from(parsed.unwrap()), expected);
        } else {
            assert_eq!(
                parsed.unwrap_err().to_string(),
                "the instant is outside the range that `SystemTime` can represent on this platform"
            );
        }
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
