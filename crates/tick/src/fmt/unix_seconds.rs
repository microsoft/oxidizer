// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt::{self, Debug, Display, Formatter};
use std::str::FromStr;
use std::time::{Duration, SystemTime};

use crate::Error;
// use crate::fmt::{Iso8601, Rfc2822};

/// A system time represented as the number of whole seconds since the Unix epoch.
///
/// Examples:
///
/// - `0` is equal to `Thu, 1 Jan 1970 00:00:00 -0000`
/// - `951786000` is equal to `Tue, 29 Feb 2000 01:00:00 -0000`
///
/// # UTC and time zones
///
/// The seconds are always represented in the UTC time zone.
///
/// # Serialization and deserialization
///
/// `UnixSeconds` implements the `Serialize` and `Deserialize` traits from the `serde` crate.
/// The system time is serialized as whole seconds. Fractional seconds are rounded down.
///
/// # Leap seconds
///
/// This value represents the number of non-leap seconds since the Unix epoch.
///
/// # Examples
///
/// ## Parsing and formatting
///
/// This example demonstrates how to parse a Unix seconds and convert it to [`SystemTime`].
///
/// ```
/// use std::time::{Duration, SystemTime};
/// use tick::fmt::UnixSeconds;
///
/// let unix_seconds = "9999".parse::<UnixSeconds>()?;
/// assert_eq!(unix_seconds.to_string(), "9999");
///
/// let system_time: SystemTime = unix_seconds.into();
/// assert_eq!(system_time, SystemTime::UNIX_EPOCH + Duration::from_secs(9999));
///
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct UnixSeconds(Duration);

impl UnixSeconds {
    /// The maximum representable value of `UnixSeconds`.
    ///
    /// This is approximately 1 billion years after the Unix epoch.
    pub const MAX: UnixSeconds = UnixSeconds(Duration::from_hours(1_000_000_000 * 365 * 24));

    /// Creates a new `UnixSeconds` from the given number of seconds since the Unix epoch.
    ///
    /// # Errors
    ///
    /// Returns an error if the provided seconds are out of range.
    ///
    /// ```
    /// use tick::fmt::UnixSeconds;
    ///
    /// UnixSeconds::from_secs(u64::MAX).unwrap_err();
    /// ```
    ///
    /// # Examples
    ///
    /// ```
    /// use std::time::{Duration, SystemTime};
    /// use tick::fmt::UnixSeconds;
    ///
    /// let unix_seconds = UnixSeconds::from_secs(10).unwrap();
    /// let system_time: SystemTime = unix_seconds.into();
    ///
    /// assert_eq!(system_time, SystemTime::UNIX_EPOCH + Duration::from_secs(10));
    /// ```
    pub fn from_secs(secs: u64) -> Result<Self, Error> {
        Self::from_duration(Duration::from_secs(secs))
    }

    /// Creates a new `UnixSeconds` from the given number of seconds since the Unix epoch.
    ///
    /// If the system time is out of range, this function returns the maximum possible system time.
    ///
    /// # Examples
    ///
    /// ```
    /// use tick::fmt::UnixSeconds;
    ///
    /// let unix_seconds = UnixSeconds::saturating_from_secs(u64::MAX);
    ///
    /// assert_eq!(unix_seconds.to_string(), "253402207200");
    /// ```
    #[must_use]
    pub fn saturating_from_secs(secs: u64) -> Self {
        Self::saturating_from_duration(Duration::from_secs(secs))
    }

    /// Creates a new `UnixSeconds` from the given duration since the Unix epoch.
    ///
    /// # Errors
    ///
    /// Returns an error if the provided duration is out of range.
    ///
    /// ```
    /// use std::time::Duration;
    /// use tick::fmt::UnixSeconds;
    ///
    /// UnixSeconds::from_duration(Duration::MAX).unwrap_err();
    /// ```
    ///
    /// # Examples
    ///
    /// ```
    /// use std::time::{Duration, SystemTime};
    /// use tick::fmt::UnixSeconds;
    ///
    /// let unix_seconds = UnixSeconds::from_duration(Duration::from_secs(10)).unwrap();
    /// let system_time: SystemTime = unix_seconds.into();
    ///
    /// assert_eq!(system_time, SystemTime::UNIX_EPOCH + Duration::from_secs(10));
    /// ```
    pub fn from_duration(duration: Duration) -> Result<Self, Error> {
        if duration > Self::MAX.0 {
            return Err(Error::out_of_range(
                "the `duration` is greater than the maximum value that can be represented by `UnixSeconds`",
            ));
        }

        Ok(Self(duration))
    }

    /// Creates a new `UnixSeconds` from the given duration since the Unix epoch.
    ///
    /// If the duration is out of range, the [`MAX`][Self::MAX] value is returned.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::time::Duration;
    /// use tick::fmt::UnixSeconds;
    ///
    /// let unix_seconds = UnixSeconds::saturating_from_duration(Duration::MAX);
    ///
    /// assert_eq!(unix_seconds.to_string(), "253402207200");
    /// ```
    #[must_use]
    pub fn saturating_from_duration(duration: Duration) -> Self {
        Self::from_duration(duration).unwrap_or(Self::MAX)
    }

    /// Returns the number of whole seconds since the Unix epoch.
    #[must_use]
    pub fn to_secs(self) -> u64 {
        self.0.as_secs()
    }
}

impl FromStr for UnixSeconds {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let secs: u64 = s.parse().map_err(Error::other)?;
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
        SystemTime::UNIX_EPOCH + value.0
    }
}

impl TryFrom<SystemTime> for UnixSeconds {
    type Error = crate::Error;

    fn try_from(value: SystemTime) -> Result<Self, Self::Error> {
        let duration = value
            .duration_since(SystemTime::UNIX_EPOCH)
            .map_err(|_| Error::out_of_range("the provided `SystemTime` is out of range and cannot be represented as `UnixSeconds`"))?;

        Self::from_duration(duration)
    }
}

// impl From<Rfc2822> for UnixSeconds {
//     fn from(value: Rfc2822) -> Self {
//         Timestamp::from(value).into()
//     }
// }

// impl From<Iso8601> for UnixSeconds {
//     fn from(value: Iso8601) -> Self {
//         Timestamp::from(value).into()
//     }
// }

#[cfg(feature = "serde")]
#[cfg_attr(docsrs, doc(cfg(feature = "serde")))]
impl serde_core::Serialize for UnixSeconds {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde_core::Serializer,
    {
        serializer.serialize_u64(self.to_secs())
    }
}

#[cfg(feature = "serde")]
#[cfg_attr(docsrs, doc(cfg(feature = "serde")))]
impl<'de> serde_core::Deserialize<'de> for UnixSeconds {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde_core::Deserializer<'de>,
    {
        let secs = u64::deserialize(deserializer).map_err(serde_core::de::Error::custom)?;
        Self::from_secs(secs).map_err(serde_core::de::Error::custom)
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_secs() {
        let ts = UnixSeconds::from_secs(10).unwrap();

        assert_eq!(ts.to_secs(), 10);
    }

    #[test]
    fn from_max_duration_err() {
        let ts = UnixSeconds::from_duration(Duration::MAX).unwrap_err();
        assert_eq!(
            ts.to_string(),
            "the `duration` is greater than the maximum value that can be represented by `UnixSeconds`"
        );
    }

    #[test]
    fn from_secs_error() {
        let error = UnixSeconds::from_secs(u64::MAX).unwrap_err();

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
    fn saturating_from_secs() {
        let ts = UnixSeconds::saturating_from_secs(u64::MAX);

        assert_eq!(ts, UnixSeconds::MAX);
    }

    #[test]
    fn parse_err() {
        "date".parse::<UnixSeconds>().unwrap_err();
    }

    #[test]
    fn parse_min() {
        let stamp: UnixSeconds = "0".parse().unwrap();
        assert_eq!(SystemTime::from(stamp), SystemTime::UNIX_EPOCH);
    }

    #[test]
    fn parse_then_display() {
        let stamp: UnixSeconds = "3600".parse().unwrap();

        // Display should return the timestamp as seconds
        assert_eq!(stamp.to_string(), "3600");
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

    #[cfg(not(miri))] // Miri is not compatible with FFI calls this needs to make.
    #[test]
    fn from_to() {
        let now = crate::Clock::with_frozen_timers().system_time();
        let iso: UnixSeconds = now.into();
        let timestamp: SystemTime = iso.into();

        assert_eq!(timestamp, now);
    }

    #[test]
    #[cfg(feature = "serde")]
    fn serialize_deserialize() {
        let iso: UnixSeconds = "9999".parse().unwrap();
        let serialized = serde_json::to_string(&iso).unwrap();
        let deserialized: UnixSeconds = serde_json::from_str(&serialized).unwrap();

        assert_eq!(iso, deserialized);
    }
}
