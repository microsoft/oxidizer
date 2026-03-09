// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

/// Attribute key for the attempt index.
#[cfg(any(feature = "metrics", test))]
pub(crate) const ATTEMPT_INDEX: &str = "resilience.attempt.index";

/// Attribute key for whether this is the last attempt.
#[cfg(any(feature = "metrics", test))]
pub(crate) const ATTEMPT_IS_LAST: &str = "resilience.attempt.is_last";

/// Attribute key for the recovery kind that triggered the attempt.
#[cfg(any(feature = "metrics", test))]
pub(crate) const ATTEMPT_RECOVERY_KIND: &str = "resilience.attempt.recovery.kind";

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
}
