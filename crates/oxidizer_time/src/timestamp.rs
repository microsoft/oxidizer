// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt::{self, Debug, Display};
use std::time::{Duration, SystemTime};

use super::{Error, Result};
use crate::fmt::Iso8601Timestamp;

/// Represents an absolute UTC point in time.
///
/// As opposed to [`std::time::Instant`], which is a representation of relative
/// time, the [`Timestamp`] represents absolute time and this value can cross
/// the process boundaries. See the [`fmt`][crate::fmt] module to see
/// timestamp formatting and parsing capabilities.
///
/// # Creation
///
/// To retrieve the current time, use [`Clock::now`][`super::Clock::now`] method.
/// Note that the retrieval of current time is not monotonic. See [`SystemTime`] for more details.
///
/// ```
/// use oxidizer_time::{Timestamp, Clock};
///
/// fn get_current_time(clock: &Clock) -> Timestamp {
///     let now = clock.now(); // Retrieve current time using the clock
///     now
/// }
/// # let clock = Clock::with_control(&oxidizer_time::ClockControl::new());
/// # let _ = get_current_time(&clock);
/// ```
///
/// Additionally, you have multiple ways to manually create a timestamp:
///
/// - [`Timestamp::from_system_time`]: Use system time to create a timestamp. This ensures
///   interoperability with other crates that support [`std::time::SystemTime`].
/// - [`fmt`][crate::fmt]: Allows parsing of timestamp from the standard formats.
///
/// ```
/// use std::time::{Duration, SystemTime};
/// use oxidizer_time::Timestamp;
///
/// let system_time = SystemTime::now();
/// let time = Timestamp::from_system_time(system_time)?;
/// assert_eq!(time.to_system_time(), system_time);
///
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
///
/// Note that the conversion from system time is fallible when system time is outside of range
/// that can be represented by the `Timestamp`.
///
/// # Formatting and parsing
///
/// The [`fmt`][crate::fmt] module is used for [`Timestamp`] timestamp parsing and
/// formatting into standard formats.
///
/// ```
/// use std::time::{SystemTime, Duration};
/// use oxidizer_time::Timestamp;
/// use oxidizer_time::fmt::Iso8601Timestamp;
///
/// let iso: Iso8601Timestamp = "1970-01-01T00:00:10Z".parse::<Iso8601Timestamp>()?;
/// let time = Timestamp::from(iso);
///
/// assert_eq!(time.to_system_time(), SystemTime::UNIX_EPOCH + Duration::from_secs(10));
///
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
///
/// # Serialization
///
/// You can serialize and deserialize the timestamp by using types in the [`fmt`][crate::fmt] module.
///
/// # Comparison
///
/// The Timestamp type provides both `Eq` and `Ord` trait implementations to facilitate
/// easy comparisons.
///
/// ```
/// use std::time::{Duration, SystemTime};
/// use oxidizer_time::Timestamp;
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
/// It returns an error in case the earlier timestamp is greater than latter.
///
/// ```
/// use std::time::{Duration, SystemTime};
/// use oxidizer_time::Timestamp;
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
/// use oxidizer_time::Timestamp;
///
/// let system_time = SystemTime::now();
/// let timestamp = Timestamp::from_system_time(system_time)?;
///
/// assert_eq!(
///     timestamp.checked_add(Duration::from_secs(5))?.to_system_time(),
///     system_time + Duration::from_secs(5));
///
/// assert_eq!(
///     timestamp.checked_sub(Duration::from_secs(5))?.to_system_time(),
///     system_time - Duration::from_secs(5));
///
/// assert!(timestamp.checked_add(Duration::MAX).is_err());
/// assert!(timestamp.checked_sub(Duration::MAX).is_err());
///
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
/// # Interoperability with [`std::time::SystemTime`]
///
/// The [`Timestamp`] can be converted to and from [`std::time::SystemTime`]. This
/// can be done by using:
///
/// - [`Timestamp::from_system_time`]: Converts the system time to a timestamp. This is
///   fallible and returns an error if the conversion overflows the timestamp.
///   The [`TryFrom<SystemTime>`] can also be used instead of explicit call.
/// - [`Timestamp::to_system_time`]: Converts the timestamp to a system time. This operation
///   never fails. The [`From<Timestamp>`] can also be used instead of explicit call.
///
/// ```
/// use std::time::{SystemTime, Duration};
/// use oxidizer_time::Timestamp;
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

    /// The maximum value that can be represented by the timestamp, i.e. `9999-30-12 22:00:00.999999999 UTC`
    pub(super) const MAX: Self = Self::new(Duration::new(253_402_207_200, 999_999_999));

    const fn new(unix_duration: Duration) -> Self {
        Self { unix_duration }
    }

    pub(super) fn now() -> Self {
        Self::from_system_time(SystemTime::now())
            .expect("conversion of system time to timestamp shall never overflow")
    }

    /// Creates a new `Timestamp` from the given [`SystemTime`]. Returns an error if the
    /// conversion of the system time to the duration overflows the timestamp.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::time::{SystemTime, Duration};
    /// use oxidizer_time::Timestamp;
    ///
    /// Timestamp::from_system_time(SystemTime::now())?;
    ///
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[expect(
        clippy::map_err_ignore,
        reason = "inner error + message is not important as we provide more relevant one"
    )]
    pub fn from_system_time(time: SystemTime) -> Result<Self> {
        let duration = time.duration_since(SystemTime::UNIX_EPOCH).map_err(|_| {
            Error::out_of_range("negative system time cannot be converted to timestamp")
        })?;

        if duration > Self::MAX.unix_duration {
            return Err(Error::out_of_range(
                "the system time is outside of valid range supported for timestamp",
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
    /// use std::time::{SystemTime, Duration};
    /// use oxidizer_time::Timestamp;
    ///
    /// let system_time = SystemTime::now();
    /// let time = Timestamp::from_system_time(system_time)?;
    /// assert_eq!(time.to_system_time(), system_time);
    ///
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    ///
    /// # Panics
    ///
    /// TODO: Document panics
    #[must_use]
    pub fn to_system_time(self) -> SystemTime {
        SystemTime::UNIX_EPOCH
            .checked_add(self.unix_duration)
            .expect("conversion of SystemTime to SystemTime::UNIX_EPOCH should never overflow")
    }

    // For some scenarios, we need to round the nano precision down by 2 numbers because it's too precise for our needs.
    // For example, we have a need for a .NET interop that cannot parse the timestamps with such high precisions.
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
    /// use oxidizer_time::Timestamp;
    ///
    /// let system_time = SystemTime::now();
    /// let now = Timestamp::from_system_time(system_time + Duration::from_secs(10))?;
    /// let earlier = Timestamp::from_system_time(system_time + Duration::from_secs(5))?;
    ///
    /// assert_eq!(now.checked_duration_since(earlier).unwrap(), Duration::from_secs(5));
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

    /// Adds the duration to the timestamp, returning an error if the overflow occurs.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::time::{Duration, SystemTime};
    /// use oxidizer_time::Timestamp;
    ///
    /// let system_time = SystemTime::now();
    /// let timestamp = Timestamp::from_system_time(system_time)?;
    /// let result = timestamp.checked_add(Duration::from_secs(5))?;
    ///
    /// assert_eq!(result.to_system_time(), system_time + Duration::from_secs(5));
    ///
    /// assert!(timestamp.checked_add(Duration::MAX).is_err()); // overflow
    ///
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn checked_add(&self, duration: Duration) -> Result<Self> {
        let duration = self.unix_duration.saturating_add(duration);

        match duration {
            _ if duration < Self::MAX.unix_duration => Ok(Self::new(duration)),
            _ => Err(Error::out_of_range(
                "adding the duration to timestamp results in a value greater than the maximum value that can be represented by timestamp",
            )),
        }
    }

    /// Subtracts the duration from the timestamp, returning an error if the resulting
    /// value would be less than minimum supported timestamp value.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::time::{Duration, SystemTime};
    /// use oxidizer_time::Timestamp;
    ///
    /// let system_time = SystemTime::now();
    /// let timestamp = Timestamp::from_system_time(system_time)?;
    /// let result = timestamp.checked_sub(Duration::from_secs(5))?;
    ///
    /// assert_eq!(result.to_system_time(), system_time - Duration::from_secs(5));
    ///
    /// assert!(timestamp.checked_sub(Duration::MAX).is_err()); // overflow
    ///
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn checked_sub(&self, duration: Duration) -> Result<Self> {
        let unix_duration = self.unix_duration.checked_sub(duration).ok_or_else(
            || {
                Error::out_of_range(
                    "subtracting the duration from timestamp results in negative value that cannot be represented by timestamp",
                )
            },
        )?;

        Ok(Self::new(unix_duration))
    }
}

/// Converts the `SystemTime` into a `Timestamp`.
impl TryFrom<SystemTime> for Timestamp {
    type Error = Error;

    fn try_from(value: SystemTime) -> Result<Self> {
        Self::from_system_time(value)
    }
}

/// Converts the `SystemTime` into a `Timestamp`.
impl From<Timestamp> for SystemTime {
    fn from(value: Timestamp) -> Self {
        value.to_system_time()
    }
}

/// Formats the `Timestamp` into a string.
///
/// The timestamp is formatted into ISO 8601 format. For example: `2024-08-21T07:04:37Z`.
/// Other formats are supported by types in the [`fmt`][crate::fmt] module.
///
/// # Examples
///
/// ```
/// use oxidizer_time::fmt::Rfc2822Timestamp;
/// use oxidizer_time::Timestamp;
///
/// let timestamp: Timestamp = "Wed, 21 Aug 2024 07:04:37 +0000".parse::<Rfc2822Timestamp>()?.into();
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oxidizer_max_is_jiff_max() {
        // In Oxidizer, we are aligning our max timestamp with Jiff's max timestamp.
        // This can prevent many conversion errors when doing Jiff interop.
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
            "the system time is outside of valid range supported for timestamp"
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
}