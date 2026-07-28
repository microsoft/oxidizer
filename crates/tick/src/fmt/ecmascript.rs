// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt::{self, Debug, Display, Formatter};
use std::str::FromStr;
use std::time::{Duration, SystemTime};

use jiff::Timestamp;
use jiff::fmt::temporal;

use crate::Error;
use crate::fmt::{Iso8601, Rfc2822, UnixSeconds};

/// Parser and formatter for the ECMAScript Date Time String Format.
///
/// For years 0000 through 9999 the output has the fixed 24-character shape
/// `YYYY-MM-DDTHH:MM:SS.sssZ`: a four-digit year, two-digit calendar and clock
/// fields, exactly three fractional digits (milliseconds, truncated rather than
/// rounded), and the UTC designator `Z`. For example: `2024-08-06T21:30:00.123Z`.
///
/// Years outside `0000..=9999` render in the ECMAScript expanded-year form - a
/// sign and six year digits, e.g. `-009999-01-02T01:59:59.000Z` - exactly as the
/// ECMAScript `Date.prototype.toISOString` method does. Such years are reachable
/// through any constructor (for example [`FromStr`]), not only by saturation, so
/// the 24-character width is a property of the `0000..=9999` range, not an
/// invariant of the type.
///
/// Within `0000..=9999` this fixed width - unlike the variable-precision output of
/// [`Iso8601`], which trims trailing fractional zeros - keeps tabular columns
/// aligned.
///
/// The format is defined by the [ECMAScript Date Time String Format](https://tc39.es/ecma262/#sec-date-time-string-format),
/// the profile produced by the ECMAScript `Date.prototype.toISOString` method.
///
/// # UTC and time zones
///
/// The ECMAScript Date Time String Format is always represented in the UTC time
/// zone with the `Z` designator.
///
/// # Parsing
///
/// Parsing accepts any [RFC 3339](https://datatracker.ietf.org/doc/html/rfc3339)
/// or ISO 8601 timestamp, of which the ECMAScript profile is a subset. Regardless
/// of the input precision, formatting always emits the fixed-width profile above.
///
/// # Serialization and deserialization
///
/// `EcmaScript` implements the `Serialize` and `Deserialize` traits from the
/// `serde_core` crate. The system time is serialized as a string using the
/// fixed-width ECMAScript Date Time String Format.
///
/// The serialization support is available when the `serde` feature is enabled.
///
/// # Range
///
/// The wrapped instant is a [`jiff::Timestamp`], whose year is in the range
/// `-9999..=9999`. Construct one fallibly with [`TryFrom`], or infallibly (with
/// out-of-range instants saturated to the nearest boundary) via
/// [`SystemTimeExt::display_ecmascript`][crate::SystemTimeExt::display_ecmascript].
/// Only the `0000..=9999` sub-range renders at the fixed 24-character width; years
/// outside it use the wider ECMAScript expanded-year form.
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
pub struct EcmaScript(pub(super) Timestamp);

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
    /// This represents a Unix system time of `9999-12-30T22:00:00.999999999Z`.
    pub const MAX: Self = Self(Timestamp::MAX);

    /// The Unix epoch represented as `EcmaScript`.
    ///
    /// This represents a Unix system time of `1 January 1970 00:00:00 UTC`.
    pub const UNIX_EPOCH: Self = Self(Timestamp::UNIX_EPOCH);

    pub(super) fn to_unix_epoch_duration(self) -> Duration {
        self.0.duration_since(Timestamp::UNIX_EPOCH).unsigned_abs()
    }

    /// Wraps a [`Timestamp`] already known to be within the representable range.
    ///
    /// Used by [`SystemTimeExt::display_ecmascript`][crate::SystemTimeExt::display_ecmascript]
    /// to build a value from an already-saturated timestamp.
    pub(crate) const fn from_timestamp(timestamp: Timestamp) -> Self {
        Self(timestamp)
    }
}

impl FromStr for EcmaScript {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let timestamp = s.parse::<jiff::Timestamp>().map_err(Error::jiff)?;
        Ok(Self(timestamp))
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
        value.0.into()
    }
}

impl TryFrom<SystemTime> for EcmaScript {
    type Error = Error;

    fn try_from(value: SystemTime) -> Result<Self, Self::Error> {
        let timestamp = Timestamp::try_from(value).map_err(Error::jiff)?;
        Ok(Self(timestamp))
    }
}

impl From<Iso8601> for EcmaScript {
    fn from(value: Iso8601) -> Self {
        Self(value.0)
    }
}

impl From<Rfc2822> for EcmaScript {
    fn from(value: Rfc2822) -> Self {
        Self(value.0)
    }
}

impl From<UnixSeconds> for EcmaScript {
    fn from(value: UnixSeconds) -> Self {
        Self(Timestamp::UNIX_EPOCH + value.0)
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
        String::deserialize(deserializer)?
            .parse::<Self>()
            .map_err(serde_core::de::Error::custom)
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use std::hash::Hash;

    use super::*;

    static_assertions::assert_impl_all!(EcmaScript: Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, TryFrom<SystemTime>, From<Iso8601>, FromStr);

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

    #[test]
    fn negative_year_renders_in_expanded_form() {
        // Years outside `0000..=9999` render in the ECMAScript expanded-year form
        // (a sign and six year digits), matching `Date.prototype.toISOString`.
        let ecma = EcmaScript::from_timestamp(Timestamp::MIN);
        let rendered = ecma.to_string();

        assert_eq!(rendered, "-009999-01-02T01:59:59.000Z");
        assert_eq!(rendered.parse::<EcmaScript>().unwrap(), ecma);

        // Expanded-year timestamps are also reachable by parsing directly.
        let parsed = "-000500-01-01T00:00:00Z".parse::<EcmaScript>().unwrap();
        assert_eq!(parsed.to_string(), "-000500-01-01T00:00:00.000Z");
    }

    #[test]
    fn unix_epoch_constant_renders_at_epoch() {
        assert_eq!(EcmaScript::UNIX_EPOCH.to_string(), "1970-01-01T00:00:00.000Z");
    }

    #[test]
    fn from_iso_8601() {
        let iso: Iso8601 = "2024-08-06T21:30:00.123456Z".parse().unwrap();
        let ecma: EcmaScript = iso.into();

        assert_eq!(ecma.to_string(), "2024-08-06T21:30:00.123Z");
    }

    #[test]
    fn from_rfc_2822() {
        let rfc: Rfc2822 = "Tue, 06 Aug 2024 21:30:00 GMT".parse().unwrap();
        let ecma: EcmaScript = rfc.into();

        assert_eq!(ecma.to_string(), "2024-08-06T21:30:00.000Z");
    }

    #[test]
    fn from_unix_seconds() {
        let unix: UnixSeconds = "3600".parse().unwrap();
        let ecma: EcmaScript = unix.into();

        assert_eq!(ecma.to_string(), "1970-01-01T01:00:00.000Z");
    }

    #[test]
    fn to_unix_epoch_duration_ok() {
        let ecma: EcmaScript = "1970-01-01T01:00:00Z".parse().unwrap();

        assert_eq!(ecma.to_unix_epoch_duration(), Duration::from_hours(1));
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
}
