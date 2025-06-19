// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt::{self, Debug, Display, Formatter};
use std::str::FromStr;
use std::time::{Duration, SystemTime};

use crate::{Error, Timestamp};

/// The format that is represented as number of whole seconds since the Unix epoch.
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
/// The `UnixSecondsTimestamp` implements the `Serialize` and `Deserialize` traits from the `serde` crate.
/// The timestamp is serialized as whole seconds. Fractional seconds are rounded down.
///
/// # Leap seconds
///
/// This value represents the number of non-leap seconds since the Unix epoch.
///
/// # Examples
///
/// ## Parsing and formatting
///
/// This example demonstrates how to parse a Unix seconds timestamp and convert it to the `Timestamp` type.
///
/// ```
/// use oxidizer_time::Timestamp;
/// use oxidizer_time::fmt::UnixSecondsTimestamp;
///
/// let unix_seconds  = "9999".parse::<UnixSecondsTimestamp>()?;
/// assert_eq!(unix_seconds.to_string(), "9999");
///
/// let timestamp: Timestamp = unix_seconds.into();
/// assert_eq!(timestamp.to_string(), "1970-01-01T02:46:39Z");
///
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct UnixSecondsTimestamp(Timestamp);

impl UnixSecondsTimestamp {
    /// Creates a new `UnixSecondsTimestamp` from the given number of seconds since the Unix epoch.
    ///
    /// # Errors
    ///
    /// Returns an error if the timestamp is out of range.
    ///
    /// ```
    /// use oxidizer_time::fmt::UnixSecondsTimestamp;
    ///
    /// UnixSecondsTimestamp::from_secs(u64::MAX).unwrap_err();
    /// ```
    ///
    /// # Examples
    ///
    /// ```
    /// use oxidizer_time::fmt::UnixSecondsTimestamp;
    /// use oxidizer_time::Timestamp;
    ///
    /// let unix_seconds = UnixSecondsTimestamp::from_secs(10).unwrap();
    /// let timestamp: Timestamp = unix_seconds.into();
    ///
    /// assert_eq!(timestamp.to_string(), "1970-01-01T00:00:10Z");
    /// ```
    pub fn from_secs(secs: u64) -> Result<Self, Error> {
        let timestamp = Timestamp::UNIX_EPOCH.checked_add(Duration::from_secs(secs))?;

        Ok(Self::from(timestamp))
    }

    /// Creates a new `UnixSecondsTimestamp` from the given number of seconds since the Unix epoch.
    ///
    /// If the timestamp is out of range, the function returns the maximum possible timestamp.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxidizer_time::fmt::UnixSecondsTimestamp;
    /// use oxidizer_time::Timestamp;
    ///
    /// let unix_seconds = UnixSecondsTimestamp::saturating_from_secs(u64::MAX);
    ///
    /// assert_eq!(unix_seconds.to_string(), "253402207200");
    ///
    /// ```
    #[must_use]
    pub fn saturating_from_secs(secs: u64) -> Self {
        Self::from_secs(secs).unwrap_or(Self(Timestamp::MAX))
    }

    /// Returns number of seconds since the Unix epoch.
    ///
    /// # Panics
    ///
    /// TODO: Document panics
    #[must_use]
    pub fn to_secs(self) -> u64 {
        self.0
            .checked_duration_since(Timestamp::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .expect("once the UnixDurationTimestamp is created, it never overflows")
    }
}

impl FromStr for UnixSecondsTimestamp {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let secs: u64 = s.parse().map_err(Error::from_other)?;
        Self::from_secs(secs)
    }
}

impl Display for UnixSecondsTimestamp {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        write!(f, "{}", self.to_secs())
    }
}

impl From<UnixSecondsTimestamp> for Timestamp {
    fn from(value: UnixSecondsTimestamp) -> Self {
        value.0
    }
}

impl From<Timestamp> for UnixSecondsTimestamp {
    fn from(value: Timestamp) -> Self {
        Self(value)
    }
}

impl From<UnixSecondsTimestamp> for SystemTime {
    fn from(value: UnixSecondsTimestamp) -> Self {
        Timestamp::from(value).to_system_time()
    }
}

#[cfg(feature = "serde")]
impl serde::Serialize for UnixSecondsTimestamp {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_u64(self.to_secs())
    }
}

#[cfg(feature = "serde")]
impl<'de> serde::Deserialize<'de> for UnixSecondsTimestamp {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let secs = u64::deserialize(deserializer).map_err(serde::de::Error::custom)?;
        Self::from_secs(secs).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_secs() {
        let ts = UnixSecondsTimestamp::from_secs(10).unwrap();

        assert_eq!(ts.to_secs(), 10);
    }

    #[test]
    fn from_secs_error() {
        let error = UnixSecondsTimestamp::from_secs(u64::MAX).unwrap_err();

        assert_eq!(
            error.to_string(),
            "adding the duration to timestamp results in a value greater than the maximum value that can be represented by timestamp"
        );
    }

    #[test]
    fn to_system_time() {
        let ts = UnixSecondsTimestamp::from_secs(10).unwrap();
        let system_time: SystemTime = ts.into();

        assert_eq!(
            system_time,
            SystemTime::UNIX_EPOCH + Duration::from_secs(10)
        );
    }

    #[test]
    fn saturating_from_secs() {
        let ts = UnixSecondsTimestamp::saturating_from_secs(u64::MAX);

        assert_eq!(ts.0, Timestamp::MAX);
    }

    #[test]
    fn parse_err() {
        "date".parse::<UnixSecondsTimestamp>().unwrap_err();
    }

    #[test]
    fn parse_min() {
        let stamp: UnixSecondsTimestamp = "0".parse().unwrap();
        assert_eq!(stamp.0.to_system_time(), SystemTime::UNIX_EPOCH);
    }

    #[test]
    fn parse_then_display() {
        let stamp: UnixSecondsTimestamp = "3600".parse().unwrap();

        // Display should return the timestamp in the RFC 2822 format
        assert_eq!(stamp.to_string(), "3600");
        assert_eq!(
            Timestamp::from(stamp),
            Timestamp::from_system_time(SystemTime::UNIX_EPOCH + Duration::from_secs(3600))
                .unwrap()
        );
    }

    #[test]
    fn parse_max() {
        let max_secs = Timestamp::MAX
            .checked_duration_since(Timestamp::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let stamp: UnixSecondsTimestamp = max_secs.to_string().parse().unwrap();
        assert_eq!(stamp.to_string(), max_secs.to_string());
    }

    #[test]
    fn parse_max_overflow() {
        "99999999999999999999999"
            .parse::<UnixSecondsTimestamp>()
            .unwrap_err();
    }

    #[cfg(not(miri))] // Miri is not compatible with FFI calls this needs to make.
    #[test]
    fn from_to() {
        let now = crate::Clock::new_dormant().now();
        let iso: UnixSecondsTimestamp = now.into();
        let timestamp: Timestamp = iso.into();

        assert_eq!(timestamp, now);
    }

    #[test]
    #[cfg(feature = "serde")]
    fn serialize_deserialize() {
        let iso: UnixSecondsTimestamp = "9999".parse().unwrap();
        let serialized = serde_json::to_string(&iso).unwrap();
        let deserialized: UnixSecondsTimestamp = serde_json::from_str(&serialized).unwrap();

        assert_eq!(iso, deserialized);
    }
}