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
    use super::*;

    #[test]
    fn private_saturated_timestamp_constructor_wraps_a_known_valid_value() {
        // `from_timestamp` is crate-private because only `SystemTimeExt` supplies saturated input.
        let ecma = EcmaScript::from_timestamp(Timestamp::UNIX_EPOCH);

        assert_eq!(ecma, EcmaScript::UNIX_EPOCH);
    }
}
