// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt::{self, Debug, Display, Formatter};
use std::str::FromStr;
use std::time::{Duration, SystemTime};

use crate::Error;

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
/// `UnixSeconds` implements the `Serialize` and `Deserialize` traits from the `serde_core` crate.
/// The system time is serialized as whole seconds. Fractional seconds are rounded down.
///
/// The serialization support is available when `serde` feature is enabled.
///
/// # Leap seconds
///
/// This value represents the number of non-leap seconds since the Unix epoch.
///
/// # Representable range
///
/// The offset from the Unix epoch is unsigned, so this type spans
/// [`UnixSeconds::UNIX_EPOCH`] (`0`) through [`UnixSeconds::MAX`] (`253402207200`). A
/// [`SystemTime`] before the Unix epoch is rejected rather than clamped.
///
/// ```
/// use std::time::{Duration, SystemTime};
///
/// use tick::fmt::UnixSeconds;
///
/// let before_epoch = SystemTime::UNIX_EPOCH - Duration::from_secs(1);
///
/// UnixSeconds::try_from(before_epoch).unwrap_err();
/// ```
///
/// # Examples
///
/// ### Parsing and formatting
///
/// This example demonstrates how to parse Unix seconds and convert them to [`SystemTime`].
///
/// ```
/// use std::time::{Duration, SystemTime};
///
/// use tick::fmt::UnixSeconds;
///
/// let unix_seconds = "9999".parse::<UnixSeconds>()?;
/// assert_eq!(unix_seconds.to_string(), "9999");
///
/// let system_time: SystemTime = unix_seconds.into();
/// assert_eq!(
///     system_time,
///     SystemTime::UNIX_EPOCH + Duration::from_secs(9999)
/// );
///
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct UnixSeconds(pub(super) Duration);

crate::thread_aware_move!(UnixSeconds);

impl UnixSeconds {
    /// The largest value that can be represented by `UnixSeconds`.
    ///
    /// This represents a Unix system time of `253402207200` seconds, which is
    /// `9999-12-30T22:00:00Z`.
    // NOTE: Kept aligned with the other formats in this module so that MAX denotes the same
    // instant everywhere.
    pub const MAX: Self = Self(Duration::new(253_402_207_200, 999_999_999));

    /// The Unix epoch represented as `UnixSeconds`.
    ///
    /// This represents a Unix system time of `1 January 1970 00:00:00 UTC`. Because the
    /// offset from the epoch is unsigned, this is also the smallest representable value.
    pub const UNIX_EPOCH: Self = Self(Duration::ZERO);

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
    ///
    /// use tick::fmt::UnixSeconds;
    ///
    /// let unix_seconds = UnixSeconds::from_secs(10).unwrap();
    /// let system_time: SystemTime = unix_seconds.into();
    ///
    /// assert_eq!(
    ///     system_time,
    ///     SystemTime::UNIX_EPOCH + Duration::from_secs(10)
    /// );
    /// ```
    pub fn from_secs(seconds: u64) -> Result<Self, Error> {
        Self::try_from(Duration::from_secs(seconds)).map_err(|_error| {
            Error::out_of_range("the `seconds` is greater than the maximum value that can be represented by `UnixSeconds`")
        })
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
        Self::UNIX_EPOCH
            .checked_add(value.0)
            .expect("`UnixSeconds::MAX` is year 9999, which every platform's `SystemTime` can represent")
    }
}

impl TryFrom<Duration> for UnixSeconds {
    type Error = Error;

    fn try_from(value: Duration) -> Result<Self, Self::Error> {
        if value > Self::MAX.0 {
            return Err(Error::out_of_range(
                "the `duration` is greater than the maximum value that can be represented by `UnixSeconds`",
            ));
        }

        Ok(Self(value))
    }
}

impl TryFrom<SystemTime> for UnixSeconds {
    type Error = crate::Error;

    fn try_from(value: SystemTime) -> Result<Self, Self::Error> {
        // Instants before the Unix epoch are rejected rather than clamped: the offset is
        // unsigned, so there is no honest way to represent them.
        Self::try_from(value.duration_since(SystemTime::UNIX_EPOCH)?)
    }
}

#[cfg(any(feature = "serde", test))]
impl serde_core::Serialize for UnixSeconds {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde_core::Serializer,
    {
        serializer.serialize_u64(self.to_secs())
    }
}

#[cfg(any(feature = "serde", test))]
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
    use std::hash::Hash;

    use jiff::Timestamp;

    use super::*;

    static_assertions::assert_impl_all!(UnixSeconds: Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, TryFrom<SystemTime>, FromStr);

    #[test]
    fn unix_epoch_is_the_smallest_value() {
        assert_eq!(UnixSeconds::UNIX_EPOCH.to_secs(), 0);
        assert_eq!(SystemTime::from(UnixSeconds::UNIX_EPOCH), SystemTime::UNIX_EPOCH);
        assert!(UnixSeconds::UNIX_EPOCH < UnixSeconds::MAX);
    }

    #[test]
    fn from_pre_epoch_system_time_is_rejected() {
        // AB#7661495: the offset is unsigned, so a pre-epoch instant has no honest
        // representation. It used to be mirrored to the same distance after the epoch.
        let before_epoch = SystemTime::UNIX_EPOCH - Duration::from_secs(1);

        UnixSeconds::try_from(before_epoch).unwrap_err();

        // The epoch itself and later instants are accepted.
        assert_eq!(UnixSeconds::try_from(SystemTime::UNIX_EPOCH).unwrap(), UnixSeconds::UNIX_EPOCH);

        let after_epoch = SystemTime::UNIX_EPOCH + Duration::from_secs(1);
        assert_eq!(UnixSeconds::try_from(after_epoch).unwrap().to_secs(), 1);
    }

    #[test]
    fn max_duration_is_jiff_duration() {
        let jiff_max = Timestamp::MAX.duration_since(Timestamp::UNIX_EPOCH).unsigned_abs();

        // equals to 123
        assert_eq!(UnixSeconds::MAX.0, jiff_max);
    }

    #[test]
    fn from_secs() {
        let ts = UnixSeconds::from_secs(10).unwrap();

        assert_eq!(ts.to_secs(), 10);
    }

    #[test]
    fn try_from_duration() {
        let ts = UnixSeconds::try_from(Duration::MAX).unwrap_err();
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
            "the `seconds` is greater than the maximum value that can be represented by `UnixSeconds`"
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
    fn round_trips_through_system_time() {
        let unix_seconds: UnixSeconds = "9999".parse().unwrap();
        let system_time: SystemTime = unix_seconds.into();

        assert_eq!(system_time, SystemTime::UNIX_EPOCH + Duration::from_secs(9999));
        assert_eq!(UnixSeconds::try_from(system_time).unwrap(), unix_seconds);

        // `SystemTime` resolution is platform defined -- 100 ns where it is FILETIME based --
        // so only whole seconds are guaranteed to survive at the sub-second maximum.
        let max = UnixSeconds::try_from(SystemTime::from(UnixSeconds::MAX)).unwrap();
        assert_eq!(max.to_secs(), UnixSeconds::MAX.to_secs());
    }

    #[test]
    fn try_from_max_ensure_accepted() {
        let unix_seconds = UnixSeconds::try_from(UnixSeconds::MAX.0).unwrap();
        assert_eq!(unix_seconds, UnixSeconds::MAX);
    }
}
