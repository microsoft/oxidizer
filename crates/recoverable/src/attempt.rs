// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt::Display;

/// Tracks the current attempt within a resilience operation.
///
/// Resilience middleware creates an `Attempt` for each execution of the inner operation and
/// passes it to user-provided callbacks. You can use the attempt information to vary behavior
/// per attempt - for example, routing to a different endpoint or injecting the attempt into
/// request extensions for downstream observability.
///
/// # Default
///
/// The [`Default`] value represents a single-shot operation with no retries:
///
/// - [`index()`](Self::index): `0` (first attempt, zero-based)
/// - [`is_first()`](Self::is_first): `true`
/// - [`is_last()`](Self::is_last): `true`
///
/// # Display
///
/// The [`Display`] implementation writes the attempt [`index()`](Self::index) as a decimal
/// number, which is useful for logging and diagnostics.
///
/// # Examples
///
/// ```
/// use recoverable::Attempt;
///
/// // First attempt of several (more attempts may follow)
/// let attempt = Attempt::new(0, false);
/// assert!(attempt.is_first());
/// assert!(!attempt.is_last());
/// assert_eq!(attempt.index(), 0);
///
/// // Final attempt (no further retries will be made)
/// let last_attempt = Attempt::new(2, true);
/// assert!(!last_attempt.is_first());
/// assert!(last_attempt.is_last());
/// assert_eq!(last_attempt.index(), 2);
///
/// // Default: single-shot, no retries
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
    /// Creates a new attempt with the given zero-based `index` and a flag indicating whether
    /// this is the last attempt the middleware will make.
    ///
    /// # Examples
    ///
    /// ```
    /// use recoverable::Attempt;
    ///
    /// let attempt = Attempt::new(0, false);
    /// assert_eq!(attempt.index(), 0);
    /// assert!(!attempt.is_last());
    /// ```
    #[must_use]
    pub fn new(index: u32, is_last: bool) -> Self {
        Self { index, is_last }
    }

    /// Returns `true` if this is the first attempt (`index == 0`).
    ///
    /// # Examples
    ///
    /// ```
    /// use recoverable::Attempt;
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

    /// Returns `true` if no further attempts will be made after this one.
    ///
    /// # Examples
    ///
    /// ```
    /// use recoverable::Attempt;
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
    /// use recoverable::Attempt;
    ///
    /// let attempt = Attempt::new(3, false);
    /// assert_eq!(attempt.index(), 3);
    /// ```
    #[must_use]
    pub fn index(self) -> u32 {
        self.index
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
    fn display_shows_index() {
        let a = Attempt::new(42, false);
        assert_eq!(format!("{a}"), "42");
    }

    #[test]
    fn default_ok() {
        let default = Attempt::default();
        assert_eq!(default.index(), 0);
        assert!(default.is_first());
        assert!(default.is_last());
    }
}
