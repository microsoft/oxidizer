// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt::{self, Debug, Display, Formatter};
use std::str::FromStr;
use std::time::SystemTime;

use jiff::{SignedDuration, Timestamp};

use crate::Error;
use crate::fmt::{Rfc2822, UnixSeconds, to_system_time};

/// Parser and formatter for system time in ISO 8601 format.
///
/// The ISO 8601 standard is used worldwide in various applications, ranging
/// from software and digital formats to international communication, ensuring
/// consistency across different systems and regions.
///
/// This type also supports parsing [RFC 3339](https://datatracker.ietf.org/doc/html/rfc3339)
/// timestamps, and the output is always compatible with both RFC 3339 and ISO 8601.
///
/// Examples:
///
/// - `2024-08-06T21:30:00Z` (UTC)
/// - `2024-08-06T14:30:00-07:00` (UTC offset)
///
/// The [`Iso8601`] format is defined in [ISO 8601](https://www.iso.org/obp/ui/#iso:std:iso:8601:-1:ed-1:v1:en).
///
/// # UTC and time zones
///
/// While ISO 8601 can include a UTC offset, the resulting [`Iso8601`] is always represented in the
/// UTC time zone with an offset of `Z`.
///
/// # Serialization and deserialization
///
/// `Iso8601` implements the `Serialize` and `Deserialize` traits from the `serde_core` crate.
/// The system time is serialized as a string using ISO 8601 format.
///
/// The serialization support is available when `serde` feature is enabled.
///
/// # Leap seconds
///
/// If an ISO 8601 string contains a leap second, parsing will succeed and the leap second will be trimmed.
///
/// ```
/// use tick::fmt::Iso8601;
///
/// let iso = "1990-12-31T23:59:60Z".parse::<Iso8601>()?;
/// assert_eq!(iso.to_string(), "1990-12-31T23:59:59Z");
///
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
///
/// # Representable range
///
/// This type spans [`Iso8601::MIN`] (`-009999-01-02T01:59:59Z`) through [`Iso8601::MAX`]
/// (`9999-12-30T22:00:00.9999999Z`), which is the widest range in the [`fmt`][crate::fmt]
/// module. Converting an `Iso8601` into a narrower format saturates to that format's nearest
/// boundary rather than producing an unrelated instant.
///
/// # Examples
///
/// ## Formatting and parsing - UTC
/// ```
/// use std::time::SystemTime;
///
/// use tick::fmt::Iso8601;
///
/// let iso = "2024-08-06T21:30:00Z".parse::<Iso8601>()?;
/// assert_eq!(iso.to_string(), "2024-08-06T21:30:00Z");
///
/// let system_time: SystemTime = iso.into();
///
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
///
/// ### Formatting and parsing - With UTC offset
///
/// This example demonstrates that the UTC offset is applied to the resulting [`Iso8601`].
/// Note that when formatting the absolute time, the UTC offset is not included in the formatted string.
/// ```
/// use std::time::SystemTime;
///
/// use tick::fmt::Iso8601;
///
/// let iso = "2024-08-06T23:30:00+02:00".parse::<Iso8601>()?;
/// assert_eq!(iso.to_string(), "2024-08-06T21:30:00Z"); // Note that the UTC offset is applied
///
/// let system_time: SystemTime = iso.into();
///
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Iso8601(pub(super) Timestamp);

crate::thread_aware_move!(Iso8601);

impl Iso8601 {
    /// The largest value that can be represented by `Iso8601`.
    ///
    /// This represents a Unix system time of `31 December 9999 23:59:59 UTC`.
    pub const MAX: Self = Self(Timestamp::MAX);

    /// The smallest value that can be represented by `Iso8601`.
    ///
    /// This represents a Unix system time of `2 January -9999 01:59:59 UTC`, expressed as an
    /// expanded ISO 8601 year.
    pub const MIN: Self = Self(Timestamp::MIN);

    /// The Unix epoch represented as `Iso8601`.
    ///
    /// This represents a Unix system time of `1 January 1970 00:00:00 UTC`.
    pub const UNIX_EPOCH: Self = Self(Timestamp::UNIX_EPOCH);
}

impl FromStr for Iso8601 {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let timestamp = s.parse::<jiff::Timestamp>().map_err(Error::jiff)?;
        Ok(Self(timestamp))
    }
}

impl Display for Iso8601 {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        // For some scenarios, we need to round the nano precision down by 2 digits because
        // it's too precise for our needs. For example, .NET interop cannot parse ISO 8601
        // timestamps with such high precision.
        let rounded = with_rounded_nanos(self.0);
        Display::fmt(&rounded, f)
    }
}

impl From<Iso8601> for SystemTime {
    fn from(value: Iso8601) -> Self {
        to_system_time(value.0.as_duration())
    }
}

impl TryFrom<SystemTime> for Iso8601 {
    type Error = Error;

    fn try_from(value: SystemTime) -> Result<Self, Self::Error> {
        let timestamp = Timestamp::try_from(value).map_err(Error::jiff)?;
        Ok(Self(timestamp))
    }
}

impl From<Rfc2822> for Iso8601 {
    fn from(value: Rfc2822) -> Self {
        Self(value.0)
    }
}

impl From<UnixSeconds> for Iso8601 {
    fn from(value: UnixSeconds) -> Self {
        Self(Timestamp::UNIX_EPOCH + value.0)
    }
}

#[cfg(any(feature = "serde", test))]
impl serde_core::Serialize for Iso8601 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde_core::Serializer,
    {
        serializer.collect_str(self)
    }
}

#[cfg(any(feature = "serde", test))]
impl<'de> serde_core::Deserialize<'de> for Iso8601 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde_core::Deserializer<'de>,
    {
        String::deserialize(deserializer)?
            .parse::<Self>()
            .map_err(serde_core::de::Error::custom)
    }
}

fn with_rounded_nanos(timestamp: Timestamp) -> Timestamp {
    let duration = timestamp.duration_since(Timestamp::UNIX_EPOCH);
    let secs = duration.as_secs();
    let nanos = duration.subsec_nanos();
    let nanos = (nanos / 100) * 100;
    let duration = SignedDuration::new(secs, nanos);

    Timestamp::UNIX_EPOCH
        .saturating_add(duration)
        .expect("this can never fail as we know we are within a valid range")
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use std::hash::Hash;
    use std::time::Duration;

    use super::*;

    static_assertions::assert_impl_all!(Iso8601: Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, TryFrom<SystemTime>, From<Iso8601>, FromStr);

    #[test]
    fn parse_err() {
        let err = "date".parse::<Iso8601>().unwrap_err();

        assert!(err.to_string().starts_with("failed to parse year in date"));
    }

    #[test]
    fn parse_min() {
        let iso: Iso8601 = "1970-01-01T00:00:00Z".parse().unwrap();

        assert_eq!(iso, Iso8601::UNIX_EPOCH);
        let system_time: SystemTime = iso.into();
        assert_eq!(system_time, SystemTime::UNIX_EPOCH);
    }

    #[test]
    fn parse_then_display() {
        let stamp: Iso8601 = "1970-01-01T01:00:00Z".parse().unwrap();

        // Display should return the timestamp in the ISO 8601 format
        assert_eq!(stamp.to_string(), "1970-01-01T01:00:00Z");
        assert_eq!(SystemTime::from(stamp), SystemTime::UNIX_EPOCH + Duration::from_hours(1));
    }

    #[test]
    fn parse_max() {
        let stamp: Iso8601 = "9999-12-30T22:00:00.9999999Z".parse().unwrap();
        assert_eq!(stamp.to_string(), "9999-12-30T22:00:00.9999999Z");
    }

    #[test]
    fn parse_max_overflow() {
        "10000-12-30T22:00:00.999999999Z".parse::<Iso8601>().unwrap_err();
    }

    #[test]
    fn from_to() {
        let at = SystemTime::UNIX_EPOCH + Duration::from_hours(1);
        let now = crate::Clock::new_frozen_at(at).system_time();

        let iso = Iso8601::try_from(now).unwrap();
        assert_eq!(SystemTime::from(iso), at);
    }

    #[test]
    fn parse_leap_seconds() {
        let stamp: Iso8601 = "1990-12-31T23:59:60Z".parse().unwrap();
        assert_eq!(stamp.to_string(), "1990-12-31T23:59:59Z");
    }

    #[test]
    #[cfg(feature = "serde")]
    fn serialize_deserialize() {
        let iso: Iso8601 = "1970-01-01T01:00:00Z".parse().unwrap();
        let serialized = serde_json::to_string(&iso).unwrap();
        let deserialized: Iso8601 = serde_json::from_str(&serialized).unwrap();

        assert_eq!(iso, deserialized);
    }

    #[test]
    fn ensure_precise_nanos_parsed() {
        let iso: Iso8601 = "1970-01-01T00:00:08.999999999Z".parse().unwrap();

        // last two nanos digits are rounded
        assert_eq!(iso.to_string(), "1970-01-01T00:00:08.9999999Z");
    }

    #[test]
    fn ensure_nanos_rounded() {
        let system_time = SystemTime::UNIX_EPOCH + Duration::new(8, 999_999_999);
        let iso: Iso8601 = system_time.try_into().unwrap();

        assert_eq!(iso.to_string(), "1970-01-01T00:00:08.9999999Z");

        let iso: Iso8601 = SystemTime::UNIX_EPOCH.try_into().unwrap();
        assert_eq!(iso.to_string(), "1970-01-01T00:00:00Z");
    }

    #[test]
    fn ensure_nanos_rounded_when_before_epoch() {
        let system_time = SystemTime::UNIX_EPOCH - Duration::new(8, 999_999_999);
        let iso: Iso8601 = system_time.try_into().unwrap();

        assert_eq!(iso.to_string(), "1969-12-31T23:59:51.0000001Z");
    }

    #[test]
    fn to_unix_seconds_saturates_before_epoch() {
        // AB#7661495: this used to mirror to `UnixSeconds(1)`, one second *after* the epoch.
        let iso: Iso8601 = "1969-12-31T23:59:59Z".parse().unwrap();

        assert_eq!(UnixSeconds::from(iso), UnixSeconds::MIN);
        assert_eq!(UnixSeconds::from(iso).to_string(), "0");

        let iso: Iso8601 = "1900-01-01T00:00:00Z".parse().unwrap();
        assert_eq!(UnixSeconds::from(iso), UnixSeconds::MIN);

        // The epoch itself and later instants are unaffected.
        assert_eq!(UnixSeconds::from(Iso8601::UNIX_EPOCH), UnixSeconds::UNIX_EPOCH);

        let iso: Iso8601 = "1970-01-01T00:00:01Z".parse().unwrap();
        assert_eq!(UnixSeconds::from(iso).to_secs(), 1);
    }

    #[test]
    fn min_is_timestamp_min() {
        assert_eq!(Iso8601::MIN.0, Timestamp::MIN);
        assert!(Iso8601::MIN < Iso8601::UNIX_EPOCH);
        assert_eq!(Iso8601::MIN.to_string(), "-009999-01-02T01:59:59Z");
    }

    #[test]
    fn to_system_time_before_filetime_epoch() {
        // AB#7663342: instants before the Windows FILETIME epoch (1601-01-01) must convert
        // without panicking, including on platforms whose `SystemTime` starts at that epoch
        // and therefore cannot represent them.
        for input in ["1000-01-01T00:00:00Z", "0001-01-01T00:00:00Z", "-000500-01-01T00:00:00Z"] {
            let iso: Iso8601 = input.parse().unwrap();
            let system_time = SystemTime::from(iso);

            assert!(system_time < SystemTime::UNIX_EPOCH, "{input} must stay before the epoch");

            match SystemTime::UNIX_EPOCH.checked_sub(iso.0.as_duration().unsigned_abs()) {
                // The platform reaches that far back, so nothing is lost.
                Some(expected) => assert_eq!(system_time, expected, "{input} must convert exactly"),
                // Otherwise the conversion saturated to the platform's own lower bound
                // instead of panicking, which every earlier instant shares.
                None => assert_eq!(system_time, SystemTime::from(Iso8601::MIN), "{input} must saturate"),
            }
        }
    }

    #[test]
    fn far_in_the_past() {
        let iso: Iso8601 = "1601-01-01T00:00:00Z".parse().unwrap();
        assert_eq!(iso.to_string(), "1601-01-01T00:00:00Z");
        assert_eq!(SystemTime::from(iso), SystemTime::UNIX_EPOCH - Duration::new(11_644_473_600, 0));
    }
}
