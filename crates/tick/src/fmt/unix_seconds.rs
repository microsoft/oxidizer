// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt::{self, Debug, Display, Formatter};
use std::str::FromStr;
use std::time::SystemTime;

use jiff::{SignedDuration, Timestamp};

use crate::Error;
use crate::fmt::{Iso8601, Rfc2822};

/// A system time represented as the number of whole seconds since the Unix epoch.
///
/// Supports both positive and negative values to represent times after and before the Unix epoch.
///
/// Examples:
///
/// - `0` is equal to `Thu, 1 Jan 1970 00:00:00 -0000`
/// - `951786000` is equal to `Tue, 29 Feb 2000 01:00:00 -0000`
/// - `-62135596800` is equal to `Mon, 1 Jan 0001 00:00:00 -0000`
///
/// # UTC and time zones
///
/// The seconds are always represented in the UTC time zone.
///
/// # Serialization and deserialization
///
/// `UnixSeconds` implements the `Serialize` and `Deserialize` traits from the `serde_core` crate.
/// The system time is serialized as whole seconds. Fractional seconds are rounded down.
///
/// The serialization support is available when `serde` feature is enabled.
///
/// # Leap seconds
///
/// This value represents the number of non-leap seconds since the Unix epoch.
///
/// # Examples
///
/// ### Parsing and formatting
///
/// This example demonstrates how to parse Unix seconds and convert them to [`SystemTime`].
///
/// ```
/// use tick::fmt::UnixSeconds;
///
/// let unix_seconds = "9999".parse::<UnixSeconds>()?;
/// assert_eq!(unix_seconds.to_string(), "9999");
///
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct UnixSeconds(pub(super) SignedDuration);

impl UnixSeconds {
    /// The largest value that be can represented by `UnixSeconds`.
    ///
    /// This represents a Unix system time of `31 December 9999 23:59:59 UTC`.
    pub const MAX: Self = Self(SignedDuration::new(253_402_207_200, 999_999_999));

    /// The smallest value that can be represented by `UnixSeconds`.
    ///
    /// This represents a Unix system time of `1 January -9999 00:00:00 UTC`.
    pub const MIN: Self = Self(SignedDuration::new(-377_705_023_201, 0));

    /// The Unix epoch represented as `UnixSeconds`.
    ///
    /// This represents a Unix system time of `1 January 1970 00:00:00 UTC` (Unix epoch).
    pub const UNIX_EPOCH: Self = Self(SignedDuration::ZERO);

    /// Creates a new `UnixSeconds` from the given number of seconds since the Unix epoch.
    ///
    /// # Errors
    ///
    /// Returns an error if the provided seconds are out of range.
    ///
    /// ```
    /// use tick::fmt::UnixSeconds;
    ///
    /// UnixSeconds::from_secs(i64::MAX).unwrap_err();
    /// ```
    ///
    /// # Examples
    ///
    /// ```
    /// use tick::fmt::UnixSeconds;
    ///
    /// let unix_seconds = UnixSeconds::from_secs(10).unwrap();
    /// assert_eq!(unix_seconds.to_secs(), 10);
    ///
    /// let negative = UnixSeconds::from_secs(-100).unwrap();
    /// assert_eq!(negative.to_secs(), -100);
    /// ```
    pub fn from_secs(seconds: i64) -> Result<Self, Error> {
        Self::try_from(SignedDuration::from_secs(seconds))
    }

    /// Returns the number of whole seconds since the Unix epoch.
    #[must_use]
    pub fn to_secs(self) -> i64 {
        self.0.as_secs()
    }
}

impl FromStr for UnixSeconds {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let secs: i64 = s.parse().map_err(Error::other)?;
        Self::from_secs(secs)
    }
}

impl Display for UnixSeconds {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        write!(f, "{}", self.to_secs())
    }
}

impl From<UnixSeconds> for SystemTime {
    fn from(value: UnixSeconds) -> Self {
        Timestamp::UNIX_EPOCH
            .saturating_add(value.0)
            .expect("UnixSeconds value is guaranteed to be within valid range")
            .into()
    }
}

impl TryFrom<SignedDuration> for UnixSeconds {
    type Error = Error;

    fn try_from(value: SignedDuration) -> Result<Self, Self::Error> {
        if value > Self::MAX.0 {
            return Err(Error::out_of_range(
                "the `duration` is greater than the maximum value that can be represented by `UnixSeconds`",
            ));
        }
        if value < Self::MIN.0 {
            return Err(Error::out_of_range(
                "the `duration` is less than the minimum value that can be represented by `UnixSeconds`",
            ));
        }

        Ok(Self(value))
    }
}

impl TryFrom<SystemTime> for UnixSeconds {
    type Error = crate::Error;

    fn try_from(value: SystemTime) -> Result<Self, Self::Error> {
        let timestamp = Timestamp::try_from(value).map_err(Error::jiff)?;
        let duration = timestamp.duration_since(Timestamp::UNIX_EPOCH);
        Self::try_from(duration)
    }
}

impl From<Rfc2822> for UnixSeconds {
    fn from(value: Rfc2822) -> Self {
        Self(value.0.duration_since(Timestamp::UNIX_EPOCH))
    }
}

impl From<Iso8601> for UnixSeconds {
    fn from(value: Iso8601) -> Self {
        Self(value.0.duration_since(Timestamp::UNIX_EPOCH))
    }
}

#[cfg(any(feature = "serde", test))]
impl serde_core::Serialize for UnixSeconds {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde_core::Serializer,
    {
        serializer.serialize_i64(self.to_secs())
    }
}

#[cfg(any(feature = "serde", test))]
impl<'de> serde_core::Deserialize<'de> for UnixSeconds {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde_core::Deserializer<'de>,
    {
        let secs = i64::deserialize(deserializer).map_err(serde_core::de::Error::custom)?;
        Self::from_secs(secs).map_err(serde_core::de::Error::custom)
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use std::hash::Hash;
    use std::time::Duration;

    use super::*;

    static_assertions::assert_impl_all!(UnixSeconds: Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, TryFrom<SystemTime>, From<Iso8601>, FromStr);

    #[test]
    fn max_duration_is_jiff_duration() {
        let jiff_max = Timestamp::MAX.duration_since(Timestamp::UNIX_EPOCH);

        assert_eq!(UnixSeconds::MAX.0, jiff_max);
    }

    #[test]
    fn min_duration_is_jiff_duration() {
        let jiff_min = Timestamp::MIN.duration_since(Timestamp::UNIX_EPOCH);

        assert_eq!(UnixSeconds::MIN.0, jiff_min);
    }

    #[test]
    fn from_secs() {
        let ts = UnixSeconds::from_secs(10).unwrap();

        assert_eq!(ts.to_secs(), 10);
    }

    #[test]
    fn from_secs_negative() {
        let ts = UnixSeconds::from_secs(-10).unwrap();

        assert_eq!(ts.to_secs(), -10);
    }

    #[test]
    fn try_from_duration() {
        let ts = UnixSeconds::try_from(SignedDuration::new(i64::MAX, 0)).unwrap_err();
        assert_eq!(
            ts.to_string(),
            "the `duration` is greater than the maximum value that can be represented by `UnixSeconds`"
        );
    }

    #[test]
    fn try_from_duration_min() {
        let ts = UnixSeconds::try_from(SignedDuration::new(i64::MIN, 0)).unwrap_err();
        assert_eq!(
            ts.to_string(),
            "the `duration` is less than the minimum value that can be represented by `UnixSeconds`"
        );
    }

    #[test]
    fn from_secs_error() {
        let error = UnixSeconds::from_secs(i64::MAX).unwrap_err();

        assert_eq!(
            error.to_string(),
            "the `duration` is greater than the maximum value that can be represented by `UnixSeconds`"
        );
    }

    #[test]
    fn to_system_time() {
        let ts = UnixSeconds::from_secs(10).unwrap();
        let system_time: SystemTime = ts.into();

        assert_eq!(system_time, SystemTime::UNIX_EPOCH + Duration::from_secs(10));
    }

    #[test]
    fn parse_err() {
        "date".parse::<UnixSeconds>().unwrap_err();
    }

    #[test]
    fn parse_unix_epoch() {
        let stamp: UnixSeconds = "0".parse().unwrap();
        assert_eq!(stamp, UnixSeconds::UNIX_EPOCH);
        assert_eq!(SystemTime::from(stamp), SystemTime::UNIX_EPOCH);
    }

    #[test]
    fn parse_negative() {
        let stamp: UnixSeconds = "-100".parse().unwrap();
        assert_eq!(stamp.to_secs(), -100);
    }

    #[test]
    fn parse_then_display() {
        let stamp: UnixSeconds = "3600".parse().unwrap();

        // Display should return the timestamp as seconds
        assert_eq!(stamp.to_string(), "3600");
    }

    #[test]
    fn parse_then_display_negative() {
        let stamp: UnixSeconds = "-3600".parse().unwrap();

        assert_eq!(stamp.to_string(), "-3600");
    }

    #[test]
    fn parse_max() {
        let max = UnixSeconds::MAX;

        let stamp: UnixSeconds = max.to_string().parse().unwrap();

        assert_eq!(stamp.to_string(), max.to_string());
    }

    #[test]
    fn parse_max_overflow() {
        "99999999999999999999999".parse::<UnixSeconds>().unwrap_err();
    }

    #[test]
    #[cfg(feature = "serde")]
    fn serialize_deserialize() {
        let iso: UnixSeconds = "9999".parse().unwrap();
        let serialized = serde_json::to_string(&iso).unwrap();
        let deserialized: UnixSeconds = serde_json::from_str(&serialized).unwrap();

        assert_eq!(iso, deserialized);
    }

    #[test]
    fn iso_8601_roundtrip() {
        let iso: UnixSeconds = "9999".parse().unwrap();
        let iso_str = iso.to_string();
        let parsed_iso: UnixSeconds = iso_str.parse().unwrap();

        assert_eq!(iso, parsed_iso);
    }

    #[test]
    fn iso_8601_roundtrip_with_timezone() {
        let unix_seconds: UnixSeconds = "9999".parse().unwrap();
        let iso: Iso8601 = unix_seconds.into();

        assert_eq!(iso.to_string(), "1970-01-01T02:46:39Z");

        let iso: Iso8601 = UnixSeconds::MAX.into();
        assert_eq!(iso, Iso8601::MAX);

        let iso: Iso8601 = UnixSeconds::MIN.into();
        assert_eq!(iso, Iso8601::MIN);

        let iso: Iso8601 = UnixSeconds::UNIX_EPOCH.into();
        assert_eq!(iso, Iso8601::UNIX_EPOCH);
    }

    #[test]
    fn try_from_max_ensure_accepted() {
        let unix_seconds = UnixSeconds::try_from(UnixSeconds::MAX.0).unwrap();
        assert_eq!(unix_seconds, UnixSeconds::MAX);
    }

    #[test]
    fn try_from_min_ensure_accepted() {
        let unix_seconds = UnixSeconds::try_from(UnixSeconds::MIN.0).unwrap();
        assert_eq!(unix_seconds, UnixSeconds::MIN);
    }
}
