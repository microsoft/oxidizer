// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt::{self, Debug, Display, Formatter};
use std::str::FromStr;
use std::time::SystemTime;

use jiff::fmt::rfc2822;

use super::utils::from_jiff;
use crate::fmt::utils::to_jiff;
use crate::{Error, Timestamp};

static RFC2822_PARSER: rfc2822::DateTimeParser = rfc2822::DateTimeParser::new();
static RFC2822_PRINTER: rfc2822::DateTimePrinter = rfc2822::DateTimePrinter::new();

/// The format used typically in HTTP headers.
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
/// While the RFC 2822 can include the UTC offset, resulting [`Timestamp`] is always represented in the
/// UTC time zone with the offset of `GMT` (zero).
///
/// # Serialization and deserialization
///
/// The `Rfc2822Timestamp` implements the `Serialize` and `Deserialize` traits from the `serde` crate.
/// The timestamp is serialized as a string using RFC 2822 format.
///
/// # Leap seconds
///
/// If the RFC 2822 string contains a leap second, the parsing will be successful and the leap seconds trimmed.
///
/// ```
/// use oxidizer_time::fmt::Rfc2822Timestamp;
///
/// let iso  = "Mon, 31 Dec 1990 23:59:60 GMT".parse::<Rfc2822Timestamp>()?;
/// assert_eq!(iso.to_string(), "Mon, 31 Dec 1990 23:59:59 GMT");
///
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
///
/// # Examples
///
/// ### Formatting and parsing - UTC
/// ```
/// use oxidizer_time::Timestamp;
/// use oxidizer_time::fmt::Rfc2822Timestamp;
///
/// let rfc  = "Tue, 06 Aug 2024 21:30:00 GMT".parse::<Rfc2822Timestamp>()?;
/// assert_eq!(rfc.to_string(), "Tue, 06 Aug 2024 21:30:00 GMT");
///
/// let timestamp: Timestamp = rfc.into();
/// assert_eq!(Rfc2822Timestamp::from(timestamp), rfc);
///
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
///
/// ### Formatting and parsing - With UTC offset
///
/// This example demonstrates that the UTC offset is applied to the resulting [`Timestamp`].
/// Notice, that when formatting the absolute time, the UTC offset is not included in the formatted string.
/// ```
/// use oxidizer_time::Timestamp;
/// use oxidizer_time::fmt::Rfc2822Timestamp;
///
/// let rfc  = "Tue, 06 Aug 2024 14:30:00 -0700".parse::<Rfc2822Timestamp>()?;
/// assert_eq!(rfc.to_string(), "Tue, 06 Aug 2024 21:30:00 GMT"); // Notice that UTC offset is applied
///
/// let timestamp: Timestamp = rfc.into();
/// assert_eq!(Rfc2822Timestamp::from(timestamp), rfc);
///
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct Rfc2822Timestamp(Timestamp);

impl FromStr for Rfc2822Timestamp {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let timestamp = from_jiff(
            RFC2822_PARSER
                .parse_timestamp(s)
                .map_err(Error::from_jiff)?,
        )?;

        Ok(Self(timestamp))
    }
}

#[expect(
    clippy::map_err_ignore,
    reason = "std::fmt::Error does not contain any data, so we ignore the inner error"
)]
impl Display for Rfc2822Timestamp {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        RFC2822_PRINTER
            .print_timestamp_rfc9110(&to_jiff(self.0), jiff::fmt::StdFmtWrite(f))
            .map_err(|_| fmt::Error)
    }
}

impl From<Rfc2822Timestamp> for Timestamp {
    fn from(value: Rfc2822Timestamp) -> Self {
        value.0
    }
}

impl From<Timestamp> for Rfc2822Timestamp {
    fn from(value: Timestamp) -> Self {
        Self(value)
    }
}

impl From<Rfc2822Timestamp> for SystemTime {
    fn from(value: Rfc2822Timestamp) -> Self {
        Timestamp::from(value).to_system_time()
    }
}

#[cfg(feature = "serde")]
impl serde::Serialize for Rfc2822Timestamp {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.collect_str(self)
    }
}

#[cfg(feature = "serde")]
impl<'de> serde::Deserialize<'de> for Rfc2822Timestamp {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        String::deserialize(deserializer)?
            .parse::<Self>()
            .map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn parse_err() {
        "date".parse::<Rfc2822Timestamp>().unwrap_err();
    }

    #[test]
    fn parse_min() {
        let stamp: Rfc2822Timestamp = "Thu, 1 Jan 1970 00:00:00 GMT".parse().unwrap();
        assert_eq!(stamp.0.to_system_time(), SystemTime::UNIX_EPOCH);
    }

    #[test]
    fn to_system_time() {
        let stamp: Rfc2822Timestamp = "Thu, 1 Jan 1970 00:00:01 GMT".parse().unwrap();
        assert_eq!(
            stamp.0.to_system_time(),
            SystemTime::UNIX_EPOCH + Duration::from_secs(1)
        );
    }

    #[test]
    fn to_system_time_alternative_format() {
        let stamp: Rfc2822Timestamp = "Thu, 1 Jan 1970 00:00:01 -0000".parse().unwrap();
        assert_eq!(
            stamp.0.to_system_time(),
            SystemTime::UNIX_EPOCH + Duration::from_secs(1)
        );
    }

    #[test]
    fn parse_then_display() {
        let stamp: Rfc2822Timestamp = "Thu, 01 Jan 1970 01:00:00 GMT".parse().unwrap();

        // Display should return the timestamp in the RFC 2822 format
        assert_eq!(stamp.to_string(), "Thu, 01 Jan 1970 01:00:00 GMT");
        assert_eq!(
            Timestamp::from(stamp),
            Timestamp::from_system_time(SystemTime::UNIX_EPOCH + Duration::from_secs(3600))
                .unwrap()
        );
    }

    #[test]
    fn parse_display_leap_year() {
        let stamp: Rfc2822Timestamp = "Tue, 29 Feb 2000 01:00:00 GMT".parse().unwrap();
        assert_eq!(stamp.to_string(), "Tue, 29 Feb 2000 01:00:00 GMT");

        let secs = Timestamp::from(stamp)
            .to_system_time()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        assert_eq!(secs, 951_786_000);
    }

    #[test]
    fn parse_max() {
        let stamp: Rfc2822Timestamp = "Thu, 30 Dec 9999 22:00:00 GMT".parse().unwrap();
        assert_eq!(stamp.to_string(), "Thu, 30 Dec 9999 22:00:00 GMT");
    }

    #[test]
    fn parse_max_overflow() {
        "Thu, 30 Dec 10000 22:00:00 GMT"
            .parse::<Rfc2822Timestamp>()
            .unwrap_err();
    }

    #[cfg(not(miri))] // Miri is not compatible with FFI calls this needs to make.
    #[test]
    fn from_to() {
        let now = crate::Clock::new_dormant().now();
        let iso: Rfc2822Timestamp = now.into();
        let timestamp: Timestamp = iso.into();

        assert_eq!(timestamp, now);
    }

    #[test]
    fn parse_leap_seconds() {
        let stamp: Rfc2822Timestamp = "Mon, 31 Dec 1990 23:59:60 GMT".parse().unwrap();
        assert_eq!(stamp.to_string(), "Mon, 31 Dec 1990 23:59:59 GMT");
    }

    #[test]
    #[cfg(feature = "serde")]
    fn serialize_deserialize() {
        let iso: Rfc2822Timestamp = "Thu, 1 Jan 1970 01:00:00 GMT".parse().unwrap();
        let serialized = serde_json::to_string(&iso).unwrap();
        let deserialized: Rfc2822Timestamp = serde_json::from_str(&serialized).unwrap();

        assert_eq!(iso, deserialized);
    }
}