// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![allow(dead_code, reason = "need to work across many combinations of features, let's just allow it")]

pub use recoverable::Attempt;

/// Attribute key for the attempt index.
pub(crate) const ATTEMPT_INDEX: &str = "resilience.attempt.index";

/// Attribute key for whether this is the last attempt.
pub(crate) const ATTEMPT_IS_LAST: &str = "resilience.attempt.is_last";

/// Attribute key for the recovery kind that triggered the attempt.
pub(crate) const ATTEMPT_RECOVERY_KIND: &str = "resilience.attempt.recovery.kind";

/// Returns the first [`Attempt`] for an operation that will make at most `max_attempts` attempts.
#[cfg(any(feature = "retry", test))]
pub(crate) fn first(max_attempts: u32) -> Attempt {
    Attempt::new(0, max_attempts == 1)
}

/// Returns the next [`Attempt`] after `attempt`, or `None` once `max_attempts` is reached.
#[cfg_attr(test, mutants::skip)] // causes test timeouts
#[cfg(any(feature = "retry", feature = "hedging", test))]
pub(crate) fn increment(attempt: Attempt, max_attempts: u32) -> Option<Attempt> {
    let next = attempt.index().saturating_add(1);

    // If we've reached or exceeded the maximum number of attempts, return None.
    if next >= max_attempts {
        return None;
    }

    let is_last = next == max_attempts.saturating_sub(1);
    Some(Attempt::new(next, is_last))
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

        let a = increment(a, max_attempts).unwrap();
        assert_eq!(a.index(), 1);
        assert!(a.is_last());

        let a = increment(a, max_attempts);
        assert!(a.is_none());
    }

    #[test]
    fn first_attempt_returns_correct_attempt() {
        let first_attempt = first(3);
        assert_eq!(first_attempt.index(), 0);
        assert!(!first_attempt.is_last());

        let first_one = first(1);
        assert_eq!(first_one.index(), 0);
        assert!(first_one.is_last());
    }
}
