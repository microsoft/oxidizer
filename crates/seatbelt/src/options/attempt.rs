// Copyright (c) Microsoft Corporation.

use std::fmt::Display;

/// Represents a single attempt in a retry operation.
///
/// This struct tracks the current attempt index, and it provides methods to check if this is the
/// first or last attempt.
///
/// The default attempt has:
/// - `attempt`: 0 (first attempt, 0-based indexing)
/// - `is_last`: true (indicating this is both the first and last attempt)
///
/// This represents a single-shot operation with no retries, where the first
/// attempt is also the final attempt.
///
/// # Examples
///
/// ```
/// use seatbelt::Attempt;
///
/// // Create the first attempt (attempt 0)
/// let attempt = Attempt::new(0, false);
/// assert!(attempt.is_first());
/// assert!(!attempt.is_last());
/// assert_eq!(attempt.index(), 0);
///
/// // Create the last attempt (attempt 2)
/// let last_attempt = Attempt::new(2, true);
/// assert!(!last_attempt.is_first());
/// assert!(last_attempt.is_last());
/// assert_eq!(last_attempt.index(), 2);
///
/// // Use the default attempt (single-shot operation)
/// let default_attempt = Attempt::default();
/// assert_eq!(default_attempt.index(), 0);
/// assert!(default_attempt.is_first());
/// assert!(default_attempt.is_last());
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Attempt {
    index: u32,
    is_last: bool,
}

impl Default for Attempt {
    fn default() -> Self {
        Self::new(0, true)
    }
}

impl Attempt {
    /// Creates a new attempt with the given attempt index and maximum attempts.
    ///
    /// # Examples
    ///
    /// ```
    /// use seatbelt::Attempt;
    ///
    /// let attempt = Attempt::new(0, false);
    /// assert_eq!(attempt.index(), 0);
    /// ```
    #[must_use]
    pub fn new(index: u32, is_last: bool) -> Self {
        Self { index, is_last }
    }

    /// Returns true if this is the first attempt (attempt 0).
    ///
    /// # Examples
    ///
    /// ```
    /// use seatbelt::Attempt;
    ///
    /// let first_attempt = Attempt::new(0, false);
    /// assert!(first_attempt.is_first());
    ///
    /// let second_attempt = Attempt::new(1, false);
    /// assert!(!second_attempt.is_first());
    /// ```
    #[must_use]
    pub fn is_first(self) -> bool {
        self.index == 0
    }

    /// Returns true if this is the last allowed attempt.
    ///
    /// # Examples
    ///
    /// ```
    /// use seatbelt::Attempt;
    ///
    /// let not_last = Attempt::new(1, false);
    /// assert!(!not_last.is_last());
    ///
    /// let last = Attempt::new(1, true);
    /// assert!(last.is_last());
    /// ```
    #[must_use]
    pub fn is_last(self) -> bool {
        self.is_last
    }

    /// Returns the current attempt index (0-based).
    ///
    /// # Examples
    ///
    /// ```
    /// use seatbelt::Attempt;
    ///
    /// let attempt = Attempt::new(3, false);
    /// assert_eq!(attempt.index(), 3);
    /// ```
    #[must_use]
    pub fn index(self) -> u32 {
        self.index
    }

    #[cfg_attr(test, mutants::skip)] // causes test timeouts
    #[cfg(any(feature = "retry", test))]
    pub(crate) fn increment(self, max_attempts: MaxAttempts) -> Option<Self> {
        let next = self.index.saturating_add(1);

        match max_attempts {
            MaxAttempts::Finite(index) => {
                // If we've reached or exceeded the maximum number of attempts, return None.
                if next >= index {
                    return None;
                }

                let is_last = next == index.saturating_sub(1);
                Some(Self::new(next, is_last))
            }
            MaxAttempts::Infinite => Some(Self::new(next, false)),
        }
    }
}

impl Display for Attempt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.index.fmt(f)
    }
}

/// Represents the maximum number of retry attempts allowed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg(any(feature = "retry", test))]
#[non_exhaustive]
pub(crate) enum MaxAttempts {
    /// A finite number of retry attempts.
    Finite(u32),

    /// An infinite number of retry attempts.
    #[cfg(any(feature = "retry", test))] // currently, only used with retry feature
    Infinite,
}

#[cfg(any(feature = "retry", test))]
impl MaxAttempts {
    #[cfg(any(feature = "retry", test))]
    pub fn first_attempt(self) -> Attempt {
        Attempt::new(0, matches!(self, Self::Finite(1)))
    }
}

#[cfg(any(feature = "retry", test))]
impl From<u32> for MaxAttempts {
    fn from(value: u32) -> Self {
        Self::Finite(value)
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_with_zero_is_first_and_not_last() {
        let a = Attempt::new(0, false);
        assert_eq!(a.index(), 0);
        assert!(a.is_first());
        assert!(!a.is_last());
    }

    #[test]
    fn new_when_equal_to_max_is_last() {
        let a = Attempt::new(5, true);
        assert!(a.is_last());
        assert!(!a.is_first());
    }

    #[test]
    fn new_with_zero_max_is_both_first_and_last() {
        let a = Attempt::new(0, true);
        assert!(a.is_first());
        assert!(a.is_last());
    }

    #[test]
    fn increment_correct_behavior() {
        let max_attempts = MaxAttempts::Finite(2);
        let a = Attempt::new(0, false);
        assert_eq!(a.index(), 0);
        assert!(!a.is_last());

        let a = a.increment(max_attempts).unwrap();
        assert_eq!(a.index(), 1);
        assert!(a.is_last());

        let a = a.increment(max_attempts);
        assert!(a.is_none());
    }

    #[test]
    fn increment_with_infinite_preserves_number() {
        let a = Attempt::new(u32::MAX, false);
        let next = a.increment(MaxAttempts::Infinite).unwrap();
        assert!(!next.is_last());
        assert_eq!(next.index(), u32::MAX);
    }

    #[test]
    fn display_shows_index() {
        let a = Attempt::new(42, false);
        assert_eq!(format!("{a}"), "42");
    }

    #[test]
    fn from_u32_to_max_attempts() {
        let finite: MaxAttempts = 5u32.into();
        assert_eq!(finite, MaxAttempts::Finite(5));

        let infinite: MaxAttempts = u32::MAX.into();
        assert_eq!(infinite, MaxAttempts::Finite(u32::MAX));
    }

    #[test]
    fn first_attempt_returns_correct_attempt() {
        let max_finite = MaxAttempts::Finite(3);
        let first = max_finite.first_attempt();
        assert_eq!(first.index(), 0);
        assert!(!first.is_last());

        let max_one = MaxAttempts::Finite(1);
        let first_one = max_one.first_attempt();
        assert_eq!(first_one.index(), 0);
        assert!(first_one.is_last());

        let max_infinite = MaxAttempts::Infinite;
        let first_infinite = max_infinite.first_attempt();
        assert_eq!(first_infinite.index(), 0);
        assert!(!first_infinite.is_last());
    }

    #[test]
    fn default_ok() {
        let default = Attempt::default();
        assert_eq!(default.index(), 0);
        assert!(default.is_first());
        assert!(default.is_last());
    }
}
