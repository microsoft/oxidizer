// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt::{self, Debug, Display, Formatter};
use std::str::FromStr;
use std::time::SystemTime;

use jiff::Timestamp;
use jiff::fmt::temporal;

use crate::Error;
use crate::fmt::ensure_system_time_representable;

/// Parser and formatter for the ECMAScript Date Time String Format.
///
/// For years 0000 through 9999 the output has the fixed 24-character shape
/// `YYYY-MM-DDTHH:MM:SS.sssZ`: a four-digit year, two-digit calendar and clock
/// fields, exactly three fractional digits (milliseconds, truncated rather than
/// rounded), and the UTC designator `Z`. For example: `2024-08-06T21:30:00.123Z`.
///
/// Years outside `0000..=9999` render in the ECMAScript expanded-year form - a
/// sign and a six-digit year, e.g. `-009999-01-02T01:59:59.000Z` - matching the
/// shape the ECMAScript `Date.prototype.toISOString` method produces. On
/// platforms whose [`SystemTime`] supports such years, they are reachable through
/// fallible constructors such as [`FromStr`]; other platforms reject them.
///
/// The format's year range is `-9999..=9999`, narrower than the
/// `-271821..=275760` range ECMAScript itself supports. The platform's
/// [`SystemTime`] range may narrow it further.
///
/// Within `0000..=9999` this fixed width - unlike the variable-precision output of
/// [`Iso8601`](crate::fmt::Iso8601), which trims trailing fractional zeros - keeps
/// tabular columns aligned.
///
/// The format is defined by the [ECMAScript Date Time String Format](https://tc39.es/ecma262/#sec-date-time-string-format),
/// the profile produced by the ECMAScript `Date.prototype.toISOString` method.
///
/// # UTC and time zones
///
/// The ECMAScript Date Time String Format is always represented in the UTC time
/// zone, using the UTC designator `Z`.
///
/// # Parsing
///
/// Parsing accepts [RFC 3339](https://datatracker.ietf.org/doc/html/rfc3339)
/// timestamps and the expanded-year syntax supported by [`jiff::Timestamp`].
/// Regardless of the input precision, formatting always emits the ECMAScript
/// profile described above.
///
/// # Serialization and deserialization
///
/// `EcmaScript` implements the `Serialize` and `Deserialize` traits from the
/// `serde_core` crate. The system time is serialized as a string using the
/// ECMAScript Date Time String Format.
///
/// The serialization support is available when the `serde` feature is enabled.
///
/// # Representable range
///
/// The upper bound is [`EcmaScript::MAX`] (`9999-12-30T22:00:00.999Z`). How far
/// back the type reaches depends on the platform: an instant is accepted only if
/// it lies within the platform's [`SystemTime`] range. This guarantees that
/// converting an `EcmaScript` back into a [`SystemTime`] always succeeds.
///
/// To format an arbitrary [`SystemTime`] without the possibility of failure -
/// saturating instants outside the supported timestamp range to the nearest
/// boundary - use
/// [`SystemTimeExt::display_ecmascript`][crate::SystemTimeExt::display_ecmascript].
/// Years outside `0000..=9999` use the wider ECMAScript expanded-year form.
///
/// # Examples
///
/// ```
/// use std::time::SystemTime;
///
/// use tick::fmt::EcmaScript;
///
/// let ecma = "2024-08-06T21:30:00Z".parse::<EcmaScript>()?;
/// assert_eq!(ecma.to_string(), "2024-08-06T21:30:00.000Z");
///
/// let system_time: SystemTime = ecma.into();
///
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EcmaScript(Timestamp);

crate::thread_aware_move!(EcmaScript);

/// Prints the fixed-width ECMAScript representation.
///
/// `precision(Some(3))` pins the fractional component to exactly three digits and
/// truncates (does not round) any finer precision. Years outside `0000..=9999` use
/// the ECMAScript expanded-year form.
static ECMASCRIPT_PRINTER: temporal::DateTimePrinter = temporal::DateTimePrinter::new().precision(Some(3));

impl EcmaScript {
    /// The largest value that can be represented by `EcmaScript`.
    ///
    /// This represents a Unix system time of `9999-12-30T22:00:00.999Z`.
    pub const MAX: Self = Self(Timestamp::constant(253_402_207_200, 999_000_000));

    /// The Unix epoch represented as `EcmaScript`.
    ///
    /// This represents a Unix system time of `1 January 1970 00:00:00 UTC`.
    pub const UNIX_EPOCH: Self = Self(Timestamp::UNIX_EPOCH);

    /// Floors `timestamp` to canonical millisecond precision.
    ///
    /// This keeps equality, ordering, hashing, [`Display`], and serde
    /// representations in agreement.
    fn canonicalize(timestamp: Timestamp) -> Timestamp {
        // Floor to the millisecond at or before the instant. Flooring (rather than
        // truncating toward zero) matches the civil-time truncation the printer
        // applies, so a pre-epoch instant keeps the same rendered fraction.
        let floored_nanos = timestamp.as_nanosecond().div_euclid(1_000_000) * 1_000_000;
        Timestamp::from_nanosecond(floored_nanos)
            .expect("flooring a valid timestamp to a millisecond boundary stays within the representable range")
    }

    /// Creates a checked canonical millisecond value.
    ///
    /// Rejects `timestamp` unless its canonical millisecond can be represented as
    /// a [`SystemTime`].
    fn new_checked(timestamp: Timestamp) -> Result<Self, Error> {
        let canonical = Self::canonicalize(timestamp);
        Ok(Self(ensure_system_time_representable(canonical)?))
    }

    /// Wraps a [`Timestamp`], truncating it to millisecond precision.
    ///
    /// Used by [`SystemTimeExt::display_ecmascript`][crate::SystemTimeExt::display_ecmascript]
    /// to build a value from an already-saturated timestamp.
    pub(crate) fn from_timestamp(timestamp: Timestamp) -> Self {
        Self::new_checked(timestamp)
            .expect("SystemTimeExt supplies either a converted SystemTime or the nearest jiff boundary, which the platform can represent")
    }
}

impl FromStr for EcmaScript {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let timestamp = s.parse::<jiff::Timestamp>().map_err(Error::jiff)?;
        Self::new_checked(timestamp)
    }
}

#[expect(
    clippy::map_err_ignore,
    reason = "std::fmt::Error does not contain any data, so we ignore the inner error"
)]
impl Display for EcmaScript {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        ECMASCRIPT_PRINTER
            .print_timestamp(&self.0, jiff::fmt::StdFmtWrite(f))
            .map_err(|_| fmt::Error)
    }
}

impl From<EcmaScript> for SystemTime {
    fn from(value: EcmaScript) -> Self {
        // Every constructor routes through `new_checked`, and the constants are
        // within the range of every platform's `SystemTime`.
        value.0.into()
    }
}

impl TryFrom<SystemTime> for EcmaScript {
    type Error = Error;

    fn try_from(value: SystemTime) -> Result<Self, Self::Error> {
        let timestamp = Timestamp::try_from(value).map_err(Error::jiff)?;
        Self::new_checked(timestamp)
    }
}

#[cfg(any(feature = "serde", test))]
impl serde_core::Serialize for EcmaScript {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde_core::Serializer,
    {
        serializer.collect_str(self)
    }
}

#[cfg(any(feature = "serde", test))]
impl<'de> serde_core::Deserialize<'de> for EcmaScript {
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

    use super::*;
    use crate::fmt::checked_system_time;

    static_assertions::assert_impl_all!(EcmaScript: Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, TryFrom<SystemTime>, FromStr);

    #[test]
    fn epoch_has_fixed_millisecond_width() {
        let epoch = EcmaScript::try_from(SystemTime::UNIX_EPOCH).unwrap();

        assert_eq!(epoch.to_string(), "1970-01-01T00:00:00.000Z");
    }

    #[test]
    fn millisecond_component_is_rendered() {
        let at = SystemTime::UNIX_EPOCH + Duration::from_millis(10_123_456);
        let formatted = EcmaScript::try_from(at).unwrap();

        assert_eq!(formatted.to_string(), "1970-01-01T02:48:43.456Z");
    }

    #[test]
    fn sub_millisecond_precision_is_truncated_not_rounded() {
        let at = SystemTime::UNIX_EPOCH + Duration::new(8, 999_999_999);
        let formatted = EcmaScript::try_from(at).unwrap();

        assert_eq!(formatted.to_string(), "1970-01-01T00:00:08.999Z");
    }

    #[test]
    fn sub_millisecond_precision_is_canonical() {
        // Two instants differing only below millisecond resolution collapse to the
        // same canonical value, so equality and Display agree.
        let a = EcmaScript::try_from(SystemTime::UNIX_EPOCH + Duration::new(8, 123_400_000)).unwrap();
        let b = EcmaScript::try_from(SystemTime::UNIX_EPOCH + Duration::new(8, 123_900_000)).unwrap();

        assert_eq!(a, b);
        assert_eq!(a.to_string(), "1970-01-01T00:00:08.123Z");
    }

    #[test]
    fn pre_epoch_sub_millisecond_floors_to_civil_millisecond() {
        // Flooring is by civil time (toward negative infinity), matching the
        // printer, not toward zero, so a pre-epoch instant keeps its fraction.
        let ecma: EcmaScript = "1969-12-31T23:59:59.999999999Z".parse().unwrap();

        assert_eq!(ecma.to_string(), "1969-12-31T23:59:59.999Z");
        assert_eq!(ecma, "1969-12-31T23:59:59.999Z".parse().unwrap());
    }

    #[test]
    fn output_width_is_constant_regardless_of_fraction() {
        let whole = EcmaScript::try_from(SystemTime::UNIX_EPOCH).unwrap();
        let fractional = EcmaScript::try_from(SystemTime::UNIX_EPOCH + Duration::from_micros(1)).unwrap();

        assert_eq!(whole.to_string().len(), fractional.to_string().len());
        assert_eq!(fractional.to_string(), "1970-01-01T00:00:00.000Z");
    }

    #[test]
    fn parse_then_display() {
        let stamp: EcmaScript = "1970-01-01T01:00:00Z".parse().unwrap();

        assert_eq!(stamp.to_string(), "1970-01-01T01:00:00.000Z");
        assert_eq!(SystemTime::from(stamp), SystemTime::UNIX_EPOCH + Duration::from_hours(1));
    }

    #[test]
    fn parse_err() {
        "date".parse::<EcmaScript>().unwrap_err();
    }

    #[test]
    fn parse_with_offset_normalizes_to_utc() {
        // Parsing accepts a numeric UTC offset; formatting normalizes to `Z`.
        let stamp: EcmaScript = "2024-08-06T14:30:00-07:00".parse().unwrap();

        assert_eq!(stamp.to_string(), "2024-08-06T21:30:00.000Z");
    }

    #[test]
    fn parse_min() {
        let stamp: EcmaScript = "1970-01-01T00:00:00Z".parse().unwrap();

        assert_eq!(stamp, EcmaScript::UNIX_EPOCH);
        assert_eq!(SystemTime::from(stamp), SystemTime::UNIX_EPOCH);
    }

    #[test]
    fn parse_max() {
        let stamp: EcmaScript = "9999-12-30T22:00:00.999999999Z".parse().unwrap();

        assert_eq!(stamp, EcmaScript::MAX);
        assert_eq!(stamp.to_string(), "9999-12-30T22:00:00.999Z");
    }

    #[test]
    fn parse_max_overflow() {
        "10000-12-30T22:00:00.999999999Z".parse::<EcmaScript>().unwrap_err();
    }

    #[test]
    fn out_of_range_conversion_is_rejected() {
        let overflow = SystemTime::from(Timestamp::MAX) + Duration::from_hours(24);

        EcmaScript::try_from(overflow).expect_err("instants beyond the representable range must be rejected");
    }

    #[test]
    fn max_constant_renders_truncated() {
        assert_eq!(EcmaScript::MAX.to_string(), "9999-12-30T22:00:00.999Z");
    }

    #[cfg(not(windows))]
    #[test]
    fn negative_year_renders_in_expanded_form() {
        // This platform can represent the full jiff range, including expanded
        // negative years.
        let ecma = EcmaScript::from_timestamp(Timestamp::MIN);
        let rendered = ecma.to_string();

        assert_eq!(rendered, "-009999-01-02T01:59:59.000Z");
        assert_eq!(rendered.parse::<EcmaScript>().unwrap(), ecma);
    }

    #[test]
    fn expanded_year_parsing_matches_system_time_range() {
        let input = "-000500-01-01T00:00:00Z";
        let timestamp: Timestamp = input.parse().unwrap();
        let parsed = input.parse::<EcmaScript>();

        match checked_system_time(timestamp) {
            Some(expected) => {
                let ecma = parsed.unwrap();
                assert_eq!(ecma.to_string(), "-000500-01-01T00:00:00.000Z");
                assert_eq!(SystemTime::from(ecma), expected);
            }
            None => {
                parsed.expect_err("the platform cannot represent this expanded-year instant");
            }
        }
    }

    #[test]
    fn unix_epoch_constant_renders_at_epoch() {
        assert_eq!(EcmaScript::UNIX_EPOCH.to_string(), "1970-01-01T00:00:00.000Z");
    }

    #[test]
    fn from_timestamp_wraps_value() {
        let ecma = EcmaScript::from_timestamp(Timestamp::UNIX_EPOCH);

        assert_eq!(ecma, EcmaScript::UNIX_EPOCH);
    }

    #[test]
    #[cfg(feature = "serde")]
    fn serialize_deserialize() {
        let ecma: EcmaScript = "1970-01-01T01:00:00Z".parse().unwrap();
        let serialized = serde_json::to_string(&ecma).unwrap();

        assert_eq!(serialized, r#""1970-01-01T01:00:00.000Z""#);

        let deserialized: EcmaScript = serde_json::from_str(&serialized).unwrap();
        assert_eq!(ecma, deserialized);
    }

    #[test]
    #[cfg(feature = "serde")]
    fn serde_round_trip_is_identity_for_sub_millisecond() {
        // Canonicalizing on construction makes serialize -> deserialize an identity,
        // even for an instant carrying sub-millisecond precision on the way in.
        let original = EcmaScript::try_from(SystemTime::UNIX_EPOCH + Duration::new(8, 123_456_789)).unwrap();
        let serialized = serde_json::to_string(&original).unwrap();
        let deserialized: EcmaScript = serde_json::from_str(&serialized).unwrap();

        assert_eq!(original, deserialized);
    }
}
