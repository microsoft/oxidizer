// Copyright (c) Microsoft Corporation.

use std::time::Duration;

/// Extension trait providing time unit conversion methods for [`Duration`].
///
/// This trait adds convenient methods to convert durations into different time units,
/// returning whole units only (fractional parts are truncated).
///
/// # Examples
///
/// ```
/// use std::time::Duration;
///
/// use tick::DurationExt;
///
/// let duration = Duration::from_secs(3661); // 1 hour, 1 minute, 1 second
/// assert_eq!(duration.as_minutes(), 61);
/// assert_eq!(duration.as_hours(), 1);
/// assert_eq!(duration.as_days(), 0);
/// ```
pub trait DurationExt {
    /// Returns the number of whole minutes in the duration.
    ///
    /// Fractional minutes are truncated (rounded down).
    ///
    /// # Examples
    ///
    /// ```
    /// use std::time::Duration;
    ///
    /// use tick::DurationExt;
    ///
    /// assert_eq!(Duration::from_secs(59).as_minutes(), 0);
    /// assert_eq!(Duration::from_secs(60).as_minutes(), 1);
    /// assert_eq!(Duration::from_secs(119).as_minutes(), 1);
    /// assert_eq!(Duration::from_secs(120).as_minutes(), 2);
    /// ```
    fn as_minutes(&self) -> u64;

    /// Returns the number of whole hours in the duration.
    ///
    /// Fractional hours are truncated (rounded down).
    ///
    /// # Examples
    ///
    /// ```
    /// use std::time::Duration;
    ///
    /// use tick::DurationExt;
    ///
    /// assert_eq!(Duration::from_secs(3599).as_hours(), 0);
    /// assert_eq!(Duration::from_secs(3600).as_hours(), 1);
    /// assert_eq!(Duration::from_secs(7199).as_hours(), 1);
    /// assert_eq!(Duration::from_secs(7200).as_hours(), 2);
    /// ```
    fn as_hours(&self) -> u64;

    /// Returns the number of whole days in the duration.
    ///
    /// Fractional days are truncated (rounded down).
    ///
    /// # Examples
    ///
    /// ```
    /// use std::time::Duration;
    ///
    /// use tick::DurationExt;
    ///
    /// assert_eq!(Duration::from_secs(86399).as_days(), 0);
    /// assert_eq!(Duration::from_secs(86400).as_days(), 1);
    /// assert_eq!(Duration::from_secs(172799).as_days(), 1);
    /// assert_eq!(Duration::from_secs(172800).as_days(), 2);
    /// ```
    fn as_days(&self) -> u64;
}

#[expect(clippy::integer_division, reason = "used operators for better clarity")]
impl DurationExt for Duration {
    fn as_minutes(&self) -> u64 {
        self.as_secs() / 60
    }

    fn as_hours(&self) -> u64 {
        self.as_minutes() / 60
    }

    fn as_days(&self) -> u64 {
        self.as_hours() / 24
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn as_minutes_ok() {
        assert_eq!(Duration::from_secs(0).as_minutes(), 0);
        assert_eq!(Duration::from_secs(1).as_minutes(), 0);
        assert_eq!(Duration::from_secs(60).as_minutes(), 1);
        assert_eq!(Duration::from_secs(61).as_minutes(), 1);
        assert_eq!(Duration::from_secs(59).as_minutes(), 0);
        assert_eq!(Duration::from_secs(80).as_minutes(), 1);
        assert_eq!(Duration::from_secs(121).as_minutes(), 2);
        assert_eq!(Duration::from_secs(u64::MAX).as_minutes(), 307_445_734_561_825_860);
    }

    #[test]
    fn as_hours_ok() {
        assert_eq!(Duration::from_secs(0).as_hours(), 0);
        assert_eq!(Duration::from_secs(1).as_hours(), 0);
        assert_eq!(Duration::from_secs(60 * 60).as_hours(), 1);
        assert_eq!(Duration::from_secs(60 * 60 + 1).as_hours(), 1);
        assert_eq!(Duration::from_secs(60 * 60 - 1).as_hours(), 0);
        assert_eq!(Duration::from_secs(2 * 60 * 60).as_hours(), 2);
        assert_eq!(Duration::from_secs(u64::MAX).as_hours(), 5_124_095_576_030_431);
    }

    #[test]
    fn as_days_ok() {
        assert_eq!(Duration::from_secs(0).as_days(), 0);
        assert_eq!(Duration::from_secs(1).as_days(), 0);
        assert_eq!(Duration::from_secs(24 * 60 * 60).as_days(), 1);
        assert_eq!(Duration::from_secs(24 * 60 * 60 + 1).as_days(), 1);
        assert_eq!(Duration::from_secs(24 * 60 * 60 - 1).as_days(), 0);
        assert_eq!(Duration::from_secs(2 * 24 * 60 * 60).as_days(), 2);
        assert_eq!(Duration::from_secs(u64::MAX).as_days(), 213_503_982_334_601);
    }
}
