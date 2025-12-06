// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt::{self, Debug, Display, Formatter};
use std::str::FromStr;
use std::time::SystemTime;

use super::utils::from_jiff;
use crate::fmt::utils::to_jiff;
use crate::{Error, Timestamp};

/// Parser and formatter for ISO 8601 timestamps.
///
/// The ISO 8601 standard is used worldwide in various applications, ranging
/// from software and digital formats to international communication, ensuring
/// consistency across different systems and regions.
///
/// Examples:
/// - `2024-08-06T21:30:00Z` (UTC)
/// - `2024-08-06T14:30:00-07:00` (UTC offset)
///
/// The [`Iso8601Timestamp`] format is defined in [ISO 8601](https://en.wikipedia.org/wiki/ISO_8601).
///
/// # UTC and time zones
///
/// While ISO 8601 can include a UTC offset, the resulting [`Timestamp`] is always represented in the
/// UTC time zone with an offset of `Z`.
///
/// # Serialization and deserialization
///
/// `Iso8601Timestamp` implements the `Serialize` and `Deserialize` traits from the `serde` crate.
/// The timestamp is serialized as a string using ISO 8601 format.
///
/// # Leap seconds
///
/// If an ISO 8601 string contains a leap second, parsing will succeed and the leap second will be trimmed.
///
/// ```
/// use tick::fmt::Iso8601Timestamp;
///
/// let iso = "1990-12-31T23:59:60Z".parse::<Iso8601Timestamp>()?;
/// assert_eq!(iso.to_string(), "1990-12-31T23:59:59Z");
///
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
///
/// # Examples
///
/// ### Formatting and parsing - UTC
/// ```
/// use tick::Timestamp;
/// use tick::fmt::Iso8601Timestamp;
///
/// let iso = "2024-08-06T21:30:00Z".parse::<Iso8601Timestamp>()?;
/// assert_eq!(iso.to_string(), "2024-08-06T21:30:00Z");
///
/// let timestamp: Timestamp = iso.into();
/// assert_eq!(Iso8601Timestamp::from(timestamp), iso);
///
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
///
/// ### Formatting and parsing - With UTC offset
///
/// This example demonstrates that the UTC offset is applied to the resulting [`Timestamp`].
/// Note that when formatting the absolute time, the UTC offset is not included in the formatted string.
/// ```
/// use tick::Timestamp;
/// use tick::fmt::Iso8601Timestamp;
///
/// let iso = "2024-08-06T23:30:00+02:00".parse::<Iso8601Timestamp>()?;
/// assert_eq!(iso.to_string(), "2024-08-06T21:30:00Z"); // Note that the UTC offset is applied
///
/// let timestamp: Timestamp = iso.into();
/// assert_eq!(Iso8601Timestamp::from(timestamp), iso);
///
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct Iso8601Timestamp(Timestamp);

impl FromStr for Iso8601Timestamp {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let timestamp = from_jiff(s.parse::<jiff::Timestamp>().map_err(Error::jiff)?)?;
        Ok(Self(timestamp))
    }
}

impl Display for Iso8601Timestamp {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        Display::fmt(&to_jiff(self.0.with_rounded_nanos()), f)
    }
}

impl From<Iso8601Timestamp> for Timestamp {
    fn from(value: Iso8601Timestamp) -> Self {
        value.0
    }
}

impl From<Timestamp> for Iso8601Timestamp {
    fn from(value: Timestamp) -> Self {
        Self(value)
    }
}

impl From<Iso8601Timestamp> for SystemTime {
    fn from(value: Iso8601Timestamp) -> Self {
        Timestamp::from(value).to_system_time()
    }
}

#[cfg(feature = "serde")]
impl serde_core::Serialize for Iso8601Timestamp {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde_core::Serializer,
    {
        serializer.collect_str(self)
    }
}

#[cfg(feature = "serde")]
impl<'de> serde_core::Deserialize<'de> for Iso8601Timestamp {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde_core::Deserializer<'de>,
    {
        String::deserialize(deserializer)?
            .parse::<Self>()
            .map_err(serde_core::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn parse_err() {
        "date".parse::<Iso8601Timestamp>().unwrap_err();
    }

    #[test]
    fn parse_min() {
        let stamp: Iso8601Timestamp = "1970-01-01T00:00:00Z".parse().unwrap();
        assert_eq!(stamp.0.to_system_time(), SystemTime::UNIX_EPOCH);
    }

    #[test]
    fn parse_then_display() {
        let stamp: Iso8601Timestamp = "1970-01-01T01:00:00Z".parse().unwrap();

        // Display should return the timestamp in the ISO 8601 format
        assert_eq!(stamp.to_string(), "1970-01-01T01:00:00Z");
        assert_eq!(
            Timestamp::from(stamp),
            Timestamp::from_system_time(SystemTime::UNIX_EPOCH + Duration::from_secs(3600)).unwrap()
        );
    }

    #[test]
    fn to_system_time() {
        let stamp: Iso8601Timestamp = "1970-01-01T01:00:00Z".parse().unwrap();
        assert_eq!(stamp.0.to_system_time(), SystemTime::UNIX_EPOCH + Duration::from_secs(3600));
    }

    #[test]
    fn parse_max() {
        let stamp: Iso8601Timestamp = "9999-12-30T22:00:00.9999999Z".parse().unwrap();
        assert_eq!(stamp.to_string(), "9999-12-30T22:00:00.9999999Z");
    }

    #[test]
    fn parse_max_overflow() {
        "10000-12-30T22:00:00.999999999Z".parse::<Iso8601Timestamp>().unwrap_err();
    }

    #[cfg(not(miri))] // Miri is not compatible with FFI calls this needs to make.
    #[test]
    fn from_to() {
        let now = crate::Clock::with_frozen_timers().timestamp();
        let iso: Iso8601Timestamp = now.into();
        let timestamp: Timestamp = iso.into();

        assert_eq!(timestamp, now);
    }

    #[test]
    fn parse_leap_seconds() {
        let stamp: Iso8601Timestamp = "1990-12-31T23:59:60Z".parse().unwrap();
        assert_eq!(stamp.to_string(), "1990-12-31T23:59:59Z");
    }

    #[test]
    #[cfg(feature = "serde")]
    fn serialize_deserialize() {
        let iso: Iso8601Timestamp = "1970-01-01T01:00:00Z".parse().unwrap();
        let serialized = serde_json::to_string(&iso).unwrap();
        let deserialized: Iso8601Timestamp = serde_json::from_str(&serialized).unwrap();

        assert_eq!(iso, deserialized);
    }

    #[test]
    fn ensure_precise_nanos_parsed() {
        let iso: Iso8601Timestamp = "1970-01-01T00:00:08.999999999Z".parse().unwrap();

        // last two nanos digits are rounded
        assert_eq!(iso.to_string(), "1970-01-01T00:00:08.9999999Z");
    }

    #[test]
    fn ensure_nanos_rounded() {
        let timestamp = Timestamp::from_system_time(SystemTime::UNIX_EPOCH + Duration::new(8, 999_999_999)).unwrap();

        let iso: Iso8601Timestamp = timestamp.into();

        assert_eq!(iso.to_string(), "1970-01-01T00:00:08.9999999Z");
    }
}
