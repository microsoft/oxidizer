// Copyright (c) Microsoft Corporation.

use std::fmt::{self, Debug, Display};
use std::ops::{Add, Sub};
use std::time::{Duration, SystemTime};

use super::{Error, Result};
use crate::fmt::Iso8601Timestamp;

/// Represents an absolute UTC point in time.
///
/// As opposed to [`std::time::Instant`], which represents relative
/// time, [`Timestamp`] represents absolute time and can cross
/// process boundaries. See the [`fmt`][crate::fmt] module for
/// timestamp formatting and parsing capabilities.
///
/// # Creation
///
/// To retrieve the current time, use the [`Clock::timestamp`][`super::Clock::timestamp`] method.
/// Note that timestamp retrieval is not monotonic. See [`SystemTime`] for more details.
///
/// ```
/// use tick::{Clock, Timestamp};
///
/// # fn get_current_time(clock: &Clock) -> Timestamp {
/// let now = clock.timestamp(); // Retrieve current time using the clock
/// now
/// # }
/// ```
///
/// Additionally, you have multiple ways to manually create a timestamp:
///
/// - [`Timestamp::from_system_time`]: Uses system time to create a timestamp. This ensures
///   interoperability with other crates that support [`std::time::SystemTime`].
/// - [`fmt`][crate::fmt]: Allows parsing timestamps from standard formats.
///
/// ```
/// use std::time::{Duration, SystemTime};
///
/// use tick::Timestamp;
///
/// let system_time = SystemTime::now();
/// let time = Timestamp::from_system_time(system_time)?;
/// assert_eq!(time.to_system_time(), system_time);
///
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
///
/// Note that conversion from system time is fallible when system time is outside the range
/// that can be represented by `Timestamp`.
///
/// # Formatting and parsing
///
/// The [`fmt`][crate::fmt] module is used for [`Timestamp`] parsing and
/// formatting into standard formats.
///
/// ```
/// use std::time::{Duration, SystemTime};
///
/// use tick::Timestamp;
/// use tick::fmt::Iso8601Timestamp;
///
/// let iso: Iso8601Timestamp = "1970-01-01T00:00:10Z".parse::<Iso8601Timestamp>()?;
/// let time = Timestamp::from(iso);
///
/// assert_eq!(
///     time.to_system_time(),
///     SystemTime::UNIX_EPOCH + Duration::from_secs(10)
/// );
///
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
///
/// # Serialization
///
/// You can serialize and deserialize timestamps using types in the [`fmt`][crate::fmt] module.
///
/// # Comparison
///
/// The Timestamp type provides both `Eq` and `Ord` trait implementations to facilitate
/// easy comparisons.
///
/// ```
/// use std::time::{Duration, SystemTime};
///
/// use tick::Timestamp;
///
/// let system_time = SystemTime::now();
/// let time1 = Timestamp::from_system_time(system_time + Duration::from_secs(1))?;
/// let time2 = Timestamp::from_system_time(system_time + Duration::from_secs(2))?;
///
/// assert!(time1 < time2);
/// assert_ne!(time1, time2);
/// assert_eq!(time1, time1);
///
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
///
/// # Arithmetic
///
/// To get a duration between two timestamps, use [`Timestamp::checked_duration_since`].
/// It returns an error if the earlier timestamp is greater than the latter.
///
/// ```
/// use std::time::{Duration, SystemTime};
///
/// use tick::Timestamp;
///
/// let system_time = SystemTime::now();
/// let time1 = Timestamp::from_system_time(system_time + Duration::from_secs(1))?;
/// let time2 = Timestamp::from_system_time(system_time + Duration::from_secs(2))?;
///
/// assert_eq!(time2.checked_duration_since(time1)?, Duration::from_secs(1));
/// assert!(time1.checked_duration_since(time2).is_err());
///
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
///
/// The Timestamp type provides arithmetic operations with [`std::time::Duration`] to
/// add and subtract time. The [`Timestamp::checked_add`] and [`Timestamp::checked_sub`]
/// return an error if the operation results in an overflow.
///
/// ```
/// use std::time::{Duration, SystemTime};
///
/// use tick::Timestamp;
///
/// let system_time = SystemTime::now();
/// let timestamp = Timestamp::from_system_time(system_time)?;
///
/// assert_eq!(
///     timestamp
///         .checked_add(Duration::from_secs(5))?
///         .to_system_time(),
///     system_time + Duration::from_secs(5)
/// );
///
/// assert_eq!(
///     timestamp
///         .checked_sub(Duration::from_secs(5))?
///         .to_system_time(),
///     system_time - Duration::from_secs(5)
/// );
///
/// assert!(timestamp.checked_add(Duration::MAX).is_err());
/// assert!(timestamp.checked_sub(Duration::MAX).is_err());
///
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
/// # Interoperability with [`std::time::SystemTime`]
///
/// [`Timestamp`] can be converted to and from [`std::time::SystemTime`]. This
/// can be done using:
///
/// - [`Timestamp::from_system_time`]: Converts system time to a timestamp. This is
///   fallible and returns an error if the conversion overflows the timestamp.
///   [`TryFrom<SystemTime>`] can also be used instead of an explicit call.
/// - [`Timestamp::to_system_time`]: Converts the timestamp to system time. This operation
///   never fails. [`From<Timestamp>`] can also be used instead of an explicit call.
///
/// ```
/// use std::time::{Duration, SystemTime};
///
/// use tick::Timestamp;
///
/// let system_time = SystemTime::now();
/// let timestamp = Timestamp::from_system_time(system_time)?;
/// assert_eq!(timestamp.to_system_time(), system_time);
///
/// let timestamp: Timestamp = Timestamp::try_from(system_time)?;
/// let system_time2: SystemTime = timestamp.into();
/// assert_eq!(system_time2, system_time);
///
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Timestamp {
    unix_duration: Duration,
}

impl Timestamp {
    pub(super) const UNIX_EPOCH: Self = Self::new(Duration::ZERO);

    /// The maximum value that can be represented by the timestamp, i.e., `9999-30-12 22:00:00.999999999 UTC`
    ///
    /// This is aligned with Jiff's maximum timestamp value to ensure interoperability.
    pub(super) const MAX: Self = Self::new(Duration::new(253_402_207_200, 999_999_999));

    const fn new(unix_duration: Duration) -> Self {
        Self { unix_duration }
    }

    /// Creates a new `Timestamp` from the given [`SystemTime`]. Returns an error if
    /// conversion of the system time to the duration overflows the timestamp.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::time::{Duration, SystemTime};
    ///
    /// use tick::Timestamp;
    ///
    /// Timestamp::from_system_time(SystemTime::now())?;
    ///
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[expect(
        clippy::map_err_ignore,
        reason = "inner error + message is not important as we provide more relevant one"
    )]
    #[cfg_attr(test, mutants::skip)] // It is infeasible to test the "beyond max" case since it is not practical to construct such a value in tests.
    pub fn from_system_time(time: SystemTime) -> Result<Self> {
        let duration = time.duration_since(SystemTime::UNIX_EPOCH).map_err(|_| {
            Error::out_of_range("negative system time cannot be converted to timestamp")
        })?;

        if duration > Self::MAX.unix_duration {
            return Err(Error::out_of_range(
                "the system time is outside the valid range supported for timestamp",
            ));
        }

        Ok(Self {
            unix_duration: duration,
        })
    }

    /// Converts the timestamp to [`SystemTime`].
    ///
    /// # Examples
    ///
    /// ```
    /// use std::time::{Duration, SystemTime};
    ///
    /// use tick::Timestamp;
    ///
    /// let system_time = SystemTime::now();
    /// let time = Timestamp::from_system_time(system_time)?;
    /// assert_eq!(time.to_system_time(), system_time);
    ///
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    #[expect(
        clippy::missing_panics_doc,
        reason = "this function is guaranteed to never panic"
    )]
    pub fn to_system_time(self) -> SystemTime {
        SystemTime::UNIX_EPOCH
            .checked_add(self.unix_duration)
            .expect("conversion of SystemTime to SystemTime::UNIX_EPOCH should never overflow")
    }

    // For some scenarios, we need to round the nano precision down by 2 digits because it's too precise for our needs.
    // For example, .NET interop cannot parse timestamps with such high precision.
    #[expect(
        clippy::arithmetic_side_effects,
        clippy::integer_division,
        reason = "We are losing precision on purpose"
    )]
    pub(super) const fn with_rounded_nanos(&self) -> Self {
        let secs = self.unix_duration.as_secs();
        let nanos = self.unix_duration.subsec_nanos();
        let nanos = (nanos / 100) * 100;
        let duration = Duration::new(secs, nanos);

        Self::new(duration)
    }

    /// Subtracts the earlier timestamp from the later timestamp, returning the [`Duration`] between them.
    ///
    /// Returns [`Error`] if the earlier timestamp is greater than the later timestamp.
    ///
    /// # Examples
    /// ```
    /// use std::time::{Duration, SystemTime};
    ///
    /// use tick::Timestamp;
    ///
    /// let system_time = SystemTime::now();
    /// let now = Timestamp::from_system_time(system_time + Duration::from_secs(10))?;
    /// let earlier = Timestamp::from_system_time(system_time + Duration::from_secs(5))?;
    ///
    /// assert_eq!(
    ///     now.checked_duration_since(earlier).unwrap(),
    ///     Duration::from_secs(5)
    /// );
    /// assert_eq!(earlier.checked_duration_since(now).is_err(), true);
    ///
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn checked_duration_since(&self, earlier: impl Into<Self>) -> Result<Duration> {
        let timestamp: Self = earlier.into();

        self.unix_duration.checked_sub(timestamp.unix_duration).ok_or_else(|| {
            Error::out_of_range("earlier timestamp refers to a point in time that is greater than the later timestamp")
        })
    }

    /// Adds the duration to the timestamp, returning an error if overflow occurs.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::time::{Duration, SystemTime};
    ///
    /// use tick::Timestamp;
    ///
    /// let system_time = SystemTime::now();
    /// let timestamp = Timestamp::from_system_time(system_time)?;
    /// let result = timestamp.checked_add(Duration::from_secs(5))?;
    ///
    /// assert_eq!(
    ///     result.to_system_time(),
    ///     system_time + Duration::from_secs(5)
    /// );
    ///
    /// assert!(timestamp.checked_add(Duration::MAX).is_err()); // overflow
    ///
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn checked_add(&self, duration: Duration) -> Result<Self> {
        let duration = self.unix_duration.saturating_add(duration);

        match duration {
            _ if duration <= Self::MAX.unix_duration => Ok(Self::new(duration)),
            _ => Err(Error::out_of_range(
                "adding the duration to timestamp results in a value greater than the maximum value that can be represented by timestamp",
            )),
        }
    }

    /// Subtracts the duration from the timestamp, returning an error if the resulting
    /// value would be less than the minimum supported timestamp value.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::time::{Duration, SystemTime};
    ///
    /// use tick::Timestamp;
    ///
    /// let system_time = SystemTime::now();
    /// let timestamp = Timestamp::from_system_time(system_time)?;
    /// let result = timestamp.checked_sub(Duration::from_secs(5))?;
    ///
    /// assert_eq!(
    ///     result.to_system_time(),
    ///     system_time - Duration::from_secs(5)
    /// );
    ///
    /// assert!(timestamp.checked_sub(Duration::MAX).is_err()); // overflow
    ///
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn checked_sub(&self, duration: Duration) -> Result<Self> {
        let unix_duration = self.unix_duration.checked_sub(duration).ok_or_else(
            || {
                Error::out_of_range(
                    "subtracting the duration from timestamp results in a negative value that cannot be represented by timestamp",
                )
            },
        )?;

        Ok(Self::new(unix_duration))
    }

    /// Adds the duration to the timestamp, saturating at the maximum timestamp value
    /// if overflow occurs.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::time::{Duration, SystemTime};
    ///
    /// use tick::Timestamp;
    ///
    /// let system_time = SystemTime::now();
    /// let timestamp = Timestamp::from_system_time(system_time)?;
    /// let result = timestamp.saturating_add(Duration::from_secs(5));
    ///
    /// assert_eq!(
    ///     result.to_system_time(),
    ///     system_time + Duration::from_secs(5)
    /// );
    ///
    /// // Saturates at the maximum value
    /// let max = timestamp.saturating_add(Duration::MAX);
    /// assert_eq!(max, max.saturating_add(Duration::from_secs(10)));
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub fn saturating_add(&self, duration: Duration) -> Self {
        self.checked_add(duration).unwrap_or(Self::MAX)
    }

    /// Subtracts the duration from the timestamp, saturating at the minimum timestamp
    /// value.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::time::{Duration, SystemTime};
    ///
    /// use tick::Timestamp;
    ///
    /// let system_time = SystemTime::now();
    /// let timestamp = Timestamp::from_system_time(system_time)?;
    /// let result = timestamp.saturating_sub(Duration::from_secs(5));
    ///
    /// assert_eq!(
    ///     result.to_system_time(),
    ///     system_time - Duration::from_secs(5)
    /// );
    ///
    /// // Saturates at the minimum value
    /// let min = timestamp.saturating_sub(Duration::MAX);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub fn saturating_sub(&self, duration: Duration) -> Self {
        self.checked_sub(duration).unwrap_or(Self::UNIX_EPOCH)
    }

    /// Subtracts the earlier timestamp from the later timestamp, returning the [`Duration`]
    /// between them, or [`Duration::ZERO`] if the earlier timestamp is greater than the
    /// later timestamp.
    ///
    /// # Examples
    /// ```
    /// use std::time::{Duration, SystemTime};
    ///
    /// use tick::Timestamp;
    ///
    /// let system_time = SystemTime::now();
    /// let now = Timestamp::from_system_time(system_time + Duration::from_secs(10))?;
    /// let earlier = Timestamp::from_system_time(system_time + Duration::from_secs(5))?;
    ///
    /// assert_eq!(
    ///     now.saturating_duration_since(earlier),
    ///     Duration::from_secs(5)
    /// );
    ///
    /// // Returns zero when earlier timestamp is greater
    /// assert_eq!(earlier.saturating_duration_since(now), Duration::ZERO);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub fn saturating_duration_since(&self, earlier: impl Into<Self>) -> Duration {
        self.checked_duration_since(earlier)
            .unwrap_or(Duration::ZERO)
    }
}

/// Converts `SystemTime` into `Timestamp`.
impl TryFrom<SystemTime> for Timestamp {
    type Error = Error;

    fn try_from(value: SystemTime) -> Result<Self> {
        Self::from_system_time(value)
    }
}

/// Converts `Timestamp` into `SystemTime`.
impl From<Timestamp> for SystemTime {
    fn from(value: Timestamp) -> Self {
        value.to_system_time()
    }
}

/// Adds a `Duration` to a `Timestamp`.
///
/// # Panics
///
/// This operation will panic if the addition would overflow the maximum timestamp value.
/// Use [`Timestamp::checked_add`] for a non-panicking alternative.
impl Add<Duration> for Timestamp {
    type Output = Self;

    fn add(self, rhs: Duration) -> Self::Output {
        self.checked_add(rhs)
            .expect("addition of duration to timestamp overflowed")
    }
}

/// Subtracts a `Duration` from a `Timestamp`.
///
/// # Panics
///
/// This operation will panic if the subtraction would underflow (result in a negative timestamp).
/// Use [`Timestamp::checked_sub`] for a non-panicking alternative.
impl Sub<Duration> for Timestamp {
    type Output = Self;

    fn sub(self, rhs: Duration) -> Self::Output {
        self.checked_sub(rhs)
            .expect("subtraction of duration from timestamp underflowed")
    }
}

/// Subtracts one `Timestamp` from another, returning the `Duration` between them.
///
/// This operation saturates at `Duration::ZERO` if the right-hand side timestamp is later than
/// the left-hand side timestamp.
impl Sub<Self> for Timestamp {
    type Output = Duration;

    fn sub(self, rhs: Self) -> Self::Output {
        self.saturating_duration_since(rhs)
    }
}

/// Formats `Timestamp` into a string.
///
/// The timestamp is formatted into ISO 8601 format. For example: `2024-08-21T07:04:37Z`.
/// Other formats are supported by types in the [`fmt`][crate::fmt] module.
///
/// # Examples
///
/// ```
/// use tick::Timestamp;
/// use tick::fmt::Rfc2822Timestamp;
///
/// let timestamp: Timestamp = "Wed, 21 Aug 2024 07:04:37 +0000"
///     .parse::<Rfc2822Timestamp>()?
///     .into();
/// assert_eq!(timestamp.to_string(), "2024-08-21T07:04:37Z");
///
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
impl Display for Timestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let iso: Iso8601Timestamp = (*self).into();
        fmt::Display::fmt(&iso, f)
    }
}

/// Serializes `Timestamp` as an ISO 8601 string.
#[cfg(feature = "serde")]
impl serde_core::Serialize for Timestamp {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde_core::Serializer,
    {
        let iso = Iso8601Timestamp::from(*self);
        iso.serialize(serializer)
    }
}

/// Deserializes `Timestamp` from an ISO 8601 string.
#[cfg(feature = "serde")]
impl<'de> serde_core::Deserialize<'de> for Timestamp {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde_core::Deserializer<'de>,
    {
        let iso = Iso8601Timestamp::deserialize(deserializer)?;
        Ok(iso.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fmt::UnixSecondsTimestamp;

    #[test]
    fn tick_max_is_jiff_max() {
        // We align our max timestamp with Jiff's max timestamp.
        // This prevents many conversion errors when doing Jiff interop.
        assert_eq!(
            Timestamp::MAX.unix_duration,
            Duration::try_from(jiff::Timestamp::MAX.as_duration()).unwrap()
        );
    }

    #[test]
    fn from_system_time_overflow() {
        let system_time =
            SystemTime::UNIX_EPOCH + Timestamp::MAX.unix_duration + Duration::from_secs(1);

        let error = Timestamp::from_system_time(system_time).unwrap_err();
        assert_eq!(
            error.to_string(),
            "the system time is outside the valid range supported for timestamp"
        );
    }

    #[test]
    fn checked_add() {
        let time =
            Timestamp::from_system_time(SystemTime::UNIX_EPOCH + Duration::from_secs(1)).unwrap();
        time.checked_add(Duration::from_secs(1)).unwrap();
        time.checked_add(Duration::MAX).unwrap_err();
    }

    #[test]
    fn checked_sub() {
        let time =
            Timestamp::from_system_time(SystemTime::UNIX_EPOCH + Duration::from_secs(2)).unwrap();
        time.checked_sub(Duration::from_secs(1)).unwrap();
        time.checked_sub(Duration::MAX).unwrap_err();
    }

    #[test]
    fn checked_sub_zero() {
        let duration = Duration::from_secs(2);

        let time = Timestamp::from_system_time(SystemTime::UNIX_EPOCH + duration).unwrap();

        assert_eq!(
            time.checked_sub(duration).unwrap().unix_duration,
            Duration::ZERO
        );
    }

    #[test]
    fn checked_duration_since() {
        let time1 =
            Timestamp::from_system_time(SystemTime::UNIX_EPOCH + Duration::from_secs(1)).unwrap();
        let time2 =
            Timestamp::from_system_time(SystemTime::UNIX_EPOCH + Duration::from_secs(2)).unwrap();

        assert_eq!(
            time2.checked_duration_since(time1).unwrap(),
            Duration::from_secs(1)
        );

        time1.checked_duration_since(time2).unwrap_err();
    }

    #[test]
    fn saturating_add() {
        let time =
            Timestamp::from_system_time(SystemTime::UNIX_EPOCH + Duration::from_secs(1)).unwrap();

        // Normal addition
        let result = time.saturating_add(Duration::from_secs(1));
        assert_eq!(result.unix_duration, Duration::from_secs(2));

        // Saturates at MAX
        let result = time.saturating_add(Duration::MAX);
        assert_eq!(result, Timestamp::MAX);

        // Adding to MAX stays at MAX
        let result = Timestamp::MAX.saturating_add(Duration::from_secs(1));
        assert_eq!(result, Timestamp::MAX);
    }

    #[test]
    fn saturating_sub() {
        let time =
            Timestamp::from_system_time(SystemTime::UNIX_EPOCH + Duration::from_secs(2)).unwrap();

        // Normal subtraction
        let result = time.saturating_sub(Duration::from_secs(1));
        assert_eq!(result.unix_duration, Duration::from_secs(1));

        // Saturates at UNIX_EPOCH
        let result = time.saturating_sub(Duration::MAX);
        assert_eq!(result, Timestamp::UNIX_EPOCH);

        // Subtracting from UNIX_EPOCH stays at UNIX_EPOCH
        let result = Timestamp::UNIX_EPOCH.saturating_sub(Duration::from_secs(1));
        assert_eq!(result, Timestamp::UNIX_EPOCH);
    }

    #[test]
    fn saturating_duration_since() {
        let time1 =
            Timestamp::from_system_time(SystemTime::UNIX_EPOCH + Duration::from_secs(1)).unwrap();
        let time2 =
            Timestamp::from_system_time(SystemTime::UNIX_EPOCH + Duration::from_secs(2)).unwrap();

        // Normal case
        assert_eq!(
            time2.saturating_duration_since(time1),
            Duration::from_secs(1)
        );

        // Returns zero when earlier is greater
        assert_eq!(time1.saturating_duration_since(time2), Duration::ZERO);

        // Same timestamps return zero
        assert_eq!(time1.saturating_duration_since(time1), Duration::ZERO);
    }

    #[test]
    fn comparison_operators() {
        let lesser =
            Timestamp::from_system_time(SystemTime::UNIX_EPOCH + Duration::from_secs(1)).unwrap();
        let greater =
            Timestamp::from_system_time(SystemTime::UNIX_EPOCH + Duration::from_secs(2)).unwrap();

        assert_ne!(lesser, greater);
        assert!(lesser <= greater);

        assert_eq!(lesser, lesser);
        assert!(lesser >= lesser);
        assert!(lesser <= lesser);

        assert!(greater >= lesser);

        assert!(lesser < greater);
        assert!(greater > lesser);
    }

    #[test]
    fn display() {
        let time =
            Timestamp::from_system_time(SystemTime::UNIX_EPOCH + Duration::from_secs(1)).unwrap();
        assert_eq!(time.to_string(), "1970-01-01T00:00:01Z");
    }

    #[test]
    fn with_rounded_nanos() {
        let duration = Duration::new(8, 999_999_899);
        let time = Timestamp::new(duration);
        let rounded = time.with_rounded_nanos();

        assert_eq!(rounded.unix_duration.as_secs(), 8);
        assert_eq!(rounded.unix_duration.subsec_nanos(), 999_999_800);
    }

    #[test]
    fn with_rounded_nanos_when_zero() {
        let duration = Duration::new(8, 0);
        let time = Timestamp::new(duration);
        let rounded = time.with_rounded_nanos();

        assert_eq!(rounded.unix_duration.as_secs(), 8);
        assert_eq!(rounded.unix_duration.subsec_nanos(), 0);
    }

    #[cfg(feature = "serde")]
    #[test]
    fn serde_with_seconds() {
        let t = "2024-02-29T12:01:59Z";
        let iso: crate::fmt::Iso8601Timestamp = t.parse().unwrap();
        let timestamp = Timestamp::from(iso);

        // Test serialization
        let serialized = serde_json::to_string(&timestamp).unwrap();
        assert_eq!(serialized, format!("\"{t}\""));

        // Test deserialization
        let deserialized: Timestamp = serde_json::from_str(&serialized).unwrap();
        assert_eq!(deserialized, timestamp);
    }

    #[cfg(feature = "serde")]
    #[test]
    fn serde_with_microseconds() {
        let t = "2024-12-31T23:59:59.456789Z";
        let iso: crate::fmt::Iso8601Timestamp = t.parse().unwrap();
        let timestamp = Timestamp::from(iso);

        let serialized = serde_json::to_string(&timestamp).unwrap();
        assert_eq!(serialized, format!("\"{t}\""));

        // Test deserialization
        let deserialized: Timestamp = serde_json::from_str(&serialized).unwrap();
        assert_eq!(deserialized, timestamp);
    }

    #[cfg(feature = "serde")]
    #[test]
    fn serde_deserialize_invalid() {
        // Test that invalid input fails to deserialize
        let result: std::result::Result<Timestamp, _> =
            serde_json::from_str("\"invalid-timestamp\"");
        assert!(
            result.is_err(),
            "Expected deserialization to fail for invalid input"
        );
    }

    #[test]
    fn add_operator() {
        let time =
            Timestamp::from_system_time(SystemTime::UNIX_EPOCH + Duration::from_secs(10)).unwrap();
        let duration = Duration::from_secs(5);

        let result = time + duration;
        assert_eq!(result.unix_duration, Duration::from_secs(15));

        // Test that it's equivalent to checked_add
        assert_eq!(result, time.checked_add(duration).unwrap());
    }

    #[test]
    fn sub_operator() {
        let time =
            Timestamp::from_system_time(SystemTime::UNIX_EPOCH + Duration::from_secs(10)).unwrap();
        let duration = Duration::from_secs(5);

        let result = time - duration;
        assert_eq!(result.unix_duration, Duration::from_secs(5));

        // Test that it's equivalent to checked_sub
        assert_eq!(result, time.checked_sub(duration).unwrap());
    }

    #[test]
    #[should_panic(expected = "addition of duration to timestamp overflowed")]
    fn add_operator_panic_on_overflow() {
        let time = Timestamp::MAX;
        let _ = time + Duration::from_secs(1);
    }

    #[test]
    #[should_panic(expected = "subtraction of duration from timestamp underflowed")]
    fn sub_operator_panic_on_underflow() {
        let time = Timestamp::UNIX_EPOCH;
        let _ = time - Duration::from_secs(1);
    }

    #[test]
    fn sub_timestamp_operator() {
        let timestamp1: Timestamp = UnixSecondsTimestamp::from_secs(1).unwrap().into();
        let timestamp2: Timestamp = UnixSecondsTimestamp::from_secs(10).unwrap().into();

        let result = timestamp2 - timestamp1;
        assert_eq!(result, Duration::from_secs(9));

        // the other way around saturates to zero
        let result = timestamp1 - timestamp2;
        assert_eq!(result, Duration::ZERO);

        // same timestamps also results in zero
        let result = timestamp1 - timestamp1;
        assert_eq!(result, Duration::ZERO);
    }
}
