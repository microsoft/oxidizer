// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt::{self, Debug, Display, Formatter};
use std::str::FromStr;
use std::time::{Duration, SystemTime};

use jiff::Timestamp;
use jiff::fmt::rfc2822;

use crate::Error;
use crate::fmt::{Iso8601, UnixSeconds};

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

    /// The Unix epoch represented as `Rfc2822`.
    ///
    /// This represents a Unix system time of `1 January 1970 00:00:00 UTC`.
    pub const UNIX_EPOCH: Self = Self(Timestamp::UNIX_EPOCH);

    pub(super) fn to_unix_epoch_duration(self) -> Duration {
        self.0.duration_since(Timestamp::UNIX_EPOCH).unsigned_abs()
    }
}

impl FromStr for Rfc2822 {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let timestamp = RFC2822_PARSER.parse_timestamp(s).map_err(Error::jiff)?;

        Ok(Self(timestamp))
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
        value.0.into()
    }
}

impl TryFrom<SystemTime> for Rfc2822 {
    type Error = Error;

    fn try_from(value: SystemTime) -> Result<Self, Self::Error> {
        let timestamp = Timestamp::try_from(value).map_err(Error::jiff)?;
        Ok(Self(timestamp))
    }
}

impl From<Iso8601> for Rfc2822 {
    fn from(value: Iso8601) -> Self {
        Self(value.0)
    }
}

impl From<UnixSeconds> for Rfc2822 {
    fn from(value: UnixSeconds) -> Self {
        Self(Timestamp::UNIX_EPOCH + value.0)
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

    use super::*;
    static_assertions::assert_impl_all!(Rfc2822: Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, TryFrom<SystemTime>, From<Iso8601>, FromStr);

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
        assert_eq!(SystemTime::from(stamp), SystemTime::UNIX_EPOCH + Duration::from_secs(3600));
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
