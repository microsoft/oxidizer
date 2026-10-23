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
/// For years 0000 through 9999, output uses the 24-character form
/// `YYYY-MM-DDTHH:MM:SS.sssZ`. It has a four-digit year, two-digit calendar and
/// clock fields, three fractional digits, and `Z`. Fractional digits are
/// truncated to milliseconds, not rounded. Example:
/// `2024-08-06T21:30:00.123Z`.
///
/// Other years use the ECMAScript expanded-year form. This form has a sign and
/// a six-digit year, such as `-009999-01-02T01:59:59.000Z`. It matches
/// `Date.prototype.toISOString`. A fallible constructor such as [`FromStr`] can
/// create these values if the platform [`SystemTime`] supports them.
///
/// This type supports years `-9999..=9999`. ECMAScript itself supports
/// `-271821..=275760`. The platform [`SystemTime`] range can reduce the range
/// of this type.
///
/// For years `0000..=9999`, fixed-width output keeps table columns aligned.
/// [`Iso8601`](crate::fmt::Iso8601) instead removes trailing fractional zeros.
///
/// The
/// [ECMAScript Date Time String Format](https://tc39.es/ecma262/#sec-date-time-string-format)
/// defines this output.
///
/// # UTC and time zones
///
/// The output always uses UTC and the `Z` designator.
///
/// # Parsing
///
/// Parsing accepts [RFC 3339](https://datatracker.ietf.org/doc/html/rfc3339)
/// timestamps. It also accepts the expanded-year syntax of [`jiff::Timestamp`].
/// Formatting always uses the ECMAScript form above.
///
/// # Serialization and deserialization
///
/// `EcmaScript` implements `Serialize` and `Deserialize` from `serde_core`.
/// Serialization writes an ECMAScript timestamp string.
///
/// Enable the `serde` feature to use this support.
///
/// # Representable range
///
/// The upper bound is [`EcmaScript::MAX`]:
/// `9999-12-30T22:00:00.999Z`. The platform defines the lower bound. Creation
/// fails if the platform [`SystemTime`] cannot represent the value. Therefore,
/// conversion back to [`SystemTime`] always succeeds.
///
/// To format any [`SystemTime`] without a conversion error, use
/// [`SystemTimeExt::display_ecmascript`][crate::SystemTimeExt::display_ecmascript].
/// It saturates times outside the timestamp range to the nearest boundary.
/// Years outside `0000..=9999` use expanded-year output.
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

/// Prints ECMAScript output with fixed millisecond precision.
///
/// `precision(Some(3))` writes three fractional digits. It truncates finer
/// precision instead of rounding it. Years outside `0000..=9999` use the
/// expanded-year form.
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

    /// Floors `timestamp` to millisecond precision.
    ///
    /// This keeps equality, ordering, hashing, [`Display`], and serialization
    /// consistent.
    fn canonicalize(timestamp: Timestamp) -> Timestamp {
        // Floor to the millisecond at or before the instant. Flooring (rather than
        // truncating toward zero) matches the civil-time truncation the printer
        // applies, so a pre-epoch instant keeps the same rendered fraction.
        let floored_nanos = timestamp.as_nanosecond().div_euclid(1_000_000) * 1_000_000;
        Timestamp::from_nanosecond(floored_nanos)
            .expect("flooring a valid timestamp to a millisecond boundary stays within the representable range")
    }

    /// Creates a checked value with millisecond precision.
    ///
    /// Returns an error if [`SystemTime`] cannot represent the result.
    fn new_checked(timestamp: Timestamp) -> Result<Self, Error> {
        let canonical = Self::canonicalize(timestamp);
        Ok(Self(ensure_system_time_representable(canonical)?))
    }

    /// Creates a value from a [`Timestamp`] and truncates it to milliseconds.
    ///
    /// [`SystemTimeExt::display_ecmascript`][crate::SystemTimeExt::display_ecmascript]
    /// uses this function after it saturates the timestamp.
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
        let timestamp = Timestamp::try_from(value).map_err(|error| Error::out_of_range(error.to_string()))?;
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
