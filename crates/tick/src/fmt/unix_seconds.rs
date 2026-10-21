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
pub struct UnixSeconds(Duration);

crate::thread_aware_move!(UnixSeconds);

impl UnixSeconds {
    /// The largest value that can be represented by `UnixSeconds`.
    ///
    /// This represents `253402207200`, or `9999-12-30T22:00:00Z`.
    #[expect(
        clippy::duration_suboptimal_units,
        reason = "UnixSeconds represents whole seconds since the Unix epoch, so seconds is the natural unit, matching to_secs() and Display"
    )]
    pub const MAX: Self = Self(Duration::from_secs(253_402_207_200));

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

    /// Drops any sub-second component so the stored value is canonical.
    ///
    /// `UnixSeconds` formats and serializes as whole seconds, so retaining only
    /// whole seconds internally keeps equality, ordering, and hashing in
    /// agreement with `Display` and serde.
    fn canonicalize(duration: Duration) -> Self {
        Self(Duration::from_secs(duration.as_secs()))
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
        let canonical = Self::canonicalize(value);
        if canonical.0 > Self::MAX.0 {
            return Err(Error::out_of_range(
                "the `duration` is greater than the maximum value that can be represented by `UnixSeconds`",
            ));
        }

        Ok(canonical)
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
