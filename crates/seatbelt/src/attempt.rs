// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![allow(dead_code, reason = "need to work across many combinations of features, let's just allow it")]

use std::fmt::Display;

/// Attribute key for the attempt index.
pub(crate) const ATTEMPT_INDEX: &str = "resilience.attempt.index";

/// Attribute key for whether this is the last attempt.
pub(crate) const ATTEMPT_IS_LAST: &str = "resilience.attempt.is_last";

/// Attribute key for the recovery kind that triggered the attempt.
pub(crate) const ATTEMPT_RECOVERY_KIND: &str = "resilience.attempt.recovery.kind";

/// Identifies one attempt in a resilience operation.
///
/// Middleware creates an `Attempt` for each call to the inner service.
/// It passes the value to user callbacks.
/// A callback can use the value to select an endpoint or add telemetry data.
///
/// # Default
///
/// The [`Default`] value represents one operation with no retries:
///
/// - [`index()`](Self::index): `0` (first attempt, zero-based)
/// - [`is_first()`](Self::is_first): `true`
/// - [`is_last()`](Self::is_last): `true`
///
/// # Display
///
/// The [`Display`] implementation writes the [`index()`](Self::index) as a decimal number.
///
/// # Examples
///
/// ```
/// use seatbelt::Attempt;
///
/// // The first attempt. More attempts can follow.
/// let attempt = Attempt::new(0, false);
/// assert!(attempt.is_first());
/// assert!(!attempt.is_last());
/// assert_eq!(attempt.index(), 0);
///
/// // The final attempt.
/// let last_attempt = Attempt::new(2, true);
/// assert!(!last_attempt.is_first());
/// assert!(last_attempt.is_last());
/// assert_eq!(last_attempt.index(), 2);
///
/// // The default represents one operation with no retries.
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
    /// Creates an attempt with a zero-based `index`.
    ///
    /// Set `is_last` to `true` when the middleware will not make another attempt.
    ///
    /// # Examples
    ///
    /// ```
    /// use seatbelt::Attempt;
    ///
    /// let attempt = Attempt::new(0, false);
    /// assert_eq!(attempt.index(), 0);
    /// assert!(!attempt.is_last());
    /// ```
    #[must_use]
    pub fn new(index: u32, is_last: bool) -> Self {
        Self { index, is_last }
    }

    /// Returns `true` when this is the first attempt.
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

    /// Returns `true` when the middleware will not make another attempt.
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

    /// Returns the zero-based attempt index.
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
    #[cfg(any(feature = "retry", feature = "hedging", test))]
    pub(crate) fn increment(self, max_attempts: u32) -> Option<Self> {
        let next = self.index.saturating_add(1);

        // If we've reached or exceeded the maximum number of attempts, return None.
        if next >= max_attempts {
            return None;
        }

        let is_last = next == max_attempts.saturating_sub(1);
        Some(Self::new(next, is_last))
    }

    #[cfg(any(feature = "retry", test))]
    pub(crate) fn first(max_attempts: u32) -> Self {
        Self::new(0, max_attempts == 1)
    }
}

impl Display for Attempt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.index.fmt(f)
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_attributes() {
        assert_eq!(ATTEMPT_INDEX, "resilience.attempt.index");
        assert_eq!(ATTEMPT_IS_LAST, "resilience.attempt.is_last");
        assert_eq!(ATTEMPT_RECOVERY_KIND, "resilience.attempt.recovery.kind");
    }

    #[test]
    fn increment_correct_behavior() {
        let max_attempts = 2;
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
    fn first_attempt_returns_correct_attempt() {
        let first = Attempt::first(3);
        assert_eq!(first.index(), 0);
        assert!(!first.is_last());

        let first_one = Attempt::first(1);
        assert_eq!(first_one.index(), 0);
        assert!(first_one.is_last());
    }
}
