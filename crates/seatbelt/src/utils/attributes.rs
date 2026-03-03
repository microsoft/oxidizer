// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

/// Key used to annotate the name of a resilience pipeline.
///
/// Values reported under this dimension should be short and concise, preferably in `snake_case`.
/// Examples: `user_auth`, `data_processing`, `payment_flow`.
#[cfg(any(feature = "metrics", test))]
pub(crate) const PIPELINE_NAME: &str = "resilience.pipeline.name";

/// Key used to annotate the name of a resilience strategy.
///
/// Values reported under this dimension should be short and concise, preferably in `snake_case`.
/// Examples: `retry`, `circuit_breaker`, `timeout`, `bulkhead`.
#[cfg(any(feature = "metrics", test))]
pub(crate) const STRATEGY_NAME: &str = "resilience.strategy.name";

/// Key used to annotate the specific resilience event being emitted.
///
/// Values reported under this dimension should be short and concise, preferably in `snake_case`.
/// Examples: `retry`, `timeout`, `circuit_opened`.
#[cfg(any(feature = "metrics", test))]
pub(crate) const EVENT_NAME: &str = "resilience.event.name";

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
    fn test_pipeline_name_is_expected() {
        assert_eq!(PIPELINE_NAME, "resilience.pipeline.name");
    }

    #[test]
    fn test_strategy_name_is_expected() {
        assert_eq!(STRATEGY_NAME, "resilience.strategy.name");
    }

    #[test]
    fn test_event_name_is_expected() {
        assert_eq!(EVENT_NAME, "resilience.event.name");
    }

    #[test]
    fn test_attempt_index_is_expected() {
        assert_eq!(ATTEMPT_INDEX, "resilience.attempt.index");
    }

    #[test]
    fn test_attempt_is_last_is_expected() {
        assert_eq!(ATTEMPT_IS_LAST, "resilience.attempt.is_last");
    }

    #[test]
    fn test_attempt_recovery_kind_is_expected() {
        assert_eq!(ATTEMPT_RECOVERY_KIND, "resilience.attempt.recovery.kind");
    }
}
