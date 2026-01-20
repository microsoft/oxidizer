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
}
