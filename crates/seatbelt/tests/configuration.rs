// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![allow(missing_docs, reason = "This is a test module")]

#[cfg(any(
    feature = "breaker",
    feature = "retry",
    feature = "timeout",
    feature = "hedging",
    feature = "chaos-latency"
))]
use std::time::Duration;

#[cfg(feature = "breaker")]
#[test]
fn breaker_defaults_match_the_public_contract() {
    use seatbelt::breaker::{AbandonedPolicy, BreakerConfig, HalfOpenMode};

    let config = BreakerConfig::default();

    assert!(config.enabled);
    assert_eq!(config.failure_threshold, 0.1);
    assert_eq!(config.min_throughput, 100);
    assert_eq!(config.sampling_duration, Duration::from_secs(30));
    assert_eq!(config.break_duration, Duration::from_secs(5));
    assert_eq!(config.half_open_mode, HalfOpenMode::progressive(None));
    assert_eq!(config.abandoned_policy, AbandonedPolicy::default());
}

#[cfg(feature = "retry")]
#[test]
fn retry_defaults_match_the_public_contract() {
    use seatbelt::retry::{Backoff, RetryConfig};

    let config = RetryConfig::default();

    assert!(config.enabled);
    assert_eq!(config.backoff_type, Backoff::Exponential);
    assert_eq!(config.base_delay, Duration::from_millis(10));
    assert_eq!(config.max_delay, None);
    assert!(config.use_jitter);
    assert_eq!(config.max_retry_attempts, 3);
    assert!(!config.handle_unavailable);
}

#[cfg(feature = "timeout")]
#[test]
fn timeout_defaults_match_the_public_contract() {
    let config = seatbelt::timeout::TimeoutConfig::default();

    assert!(config.enabled);
    assert_eq!(config.timeout, Duration::from_secs(30));
}

#[cfg(feature = "hedging")]
#[test]
fn hedging_defaults_match_the_public_contract() {
    let config = seatbelt::hedging::HedgingConfig::default();

    assert!(config.enabled);
    assert_eq!(config.hedging_delay, Duration::from_millis(500));
    assert_eq!(config.max_hedged_attempts, 1);
    assert!(!config.handle_unavailable);
}

#[cfg(feature = "chaos-injection")]
#[test]
fn injection_defaults_match_the_public_contract() {
    let config = seatbelt::chaos::injection::InjectionConfig::default();

    assert!(config.enabled);
    assert_eq!(config.rate, 0.0);
}

#[cfg(feature = "chaos-latency")]
#[test]
fn latency_defaults_match_the_public_contract() {
    let config = seatbelt::chaos::latency::LatencyConfig::default();

    assert!(config.enabled);
    assert_eq!(config.rate, 0.0);
    assert_eq!(config.latency, Duration::ZERO);
    assert_eq!(config.max_latency, None);
}

#[cfg(all(feature = "serde", feature = "breaker"))]
#[test]
fn breaker_configuration_serialization_roundtrips() {
    use seatbelt::breaker::{AbandonedPolicy, BreakerConfig, HalfOpenMode};

    let default = BreakerConfig::default();
    assert_eq!(
        serde_json::to_string(&default).unwrap(),
        r#"{"enabled":true,"failure_threshold":0.1,"min_throughput":100,"sampling_duration":"30s","break_duration":"5s","half_open_mode":{"progressive":null},"abandoned_policy":{"rate_threshold":1.0}}"#
    );

    let mut quick = default.clone();
    quick.half_open_mode = HalfOpenMode::quick();
    assert_eq!(
        serde_json::to_string(&quick).unwrap(),
        r#"{"enabled":true,"failure_threshold":0.1,"min_throughput":100,"sampling_duration":"30s","break_duration":"5s","half_open_mode":"quick","abandoned_policy":{"rate_threshold":1.0}}"#
    );

    let mut progressive = default.clone();
    progressive.half_open_mode = HalfOpenMode::progressive(Duration::from_secs(45));
    assert_eq!(
        serde_json::to_string(&progressive).unwrap(),
        r#"{"enabled":true,"failure_threshold":0.1,"min_throughput":100,"sampling_duration":"30s","break_duration":"5s","half_open_mode":{"progressive":"45s"},"abandoned_policy":{"rate_threshold":1.0}}"#
    );

    let mut config = default;
    config.enabled = false;
    config.failure_threshold = 0.25;
    config.min_throughput = 20;
    config.sampling_duration = Duration::from_secs(45);
    config.break_duration = Duration::from_secs(10);
    config.half_open_mode = HalfOpenMode::quick();
    config.abandoned_policy = AbandonedPolicy::as_failures();

    let json = serde_json::to_string(&config).unwrap();
    assert_eq!(serde_json::from_str::<BreakerConfig>(&json).unwrap(), config);
}

#[cfg(all(feature = "serde", feature = "retry"))]
#[test]
fn retry_configuration_serialization_roundtrips() {
    use seatbelt::retry::{Backoff, RetryConfig};

    let default = RetryConfig::default();
    assert_eq!(
        serde_json::to_string(&default).unwrap(),
        r#"{"enabled":true,"backoff_type":"exponential","base_delay":"10ms","max_delay":null,"use_jitter":true,"max_retry_attempts":3,"handle_unavailable":false}"#
    );

    let mut config = default;
    config.enabled = false;
    config.backoff_type = Backoff::Linear;
    config.base_delay = Duration::from_secs(2);
    config.max_delay = Some(Duration::from_mins(1));
    config.use_jitter = false;
    config.max_retry_attempts = 7;
    config.handle_unavailable = true;

    let json = serde_json::to_string(&config).unwrap();
    assert_eq!(serde_json::from_str::<RetryConfig>(&json).unwrap(), config);
}

#[cfg(all(feature = "serde", feature = "timeout"))]
#[test]
fn timeout_configuration_serialization_roundtrips() {
    let default = seatbelt::timeout::TimeoutConfig::default();
    assert_eq!(serde_json::to_string(&default).unwrap(), r#"{"enabled":true,"timeout":"30s"}"#);

    let mut config = default;
    config.enabled = false;
    config.timeout = Duration::from_secs(45);

    let json = serde_json::to_string(&config).unwrap();
    assert_eq!(serde_json::from_str::<seatbelt::timeout::TimeoutConfig>(&json).unwrap(), config);
}

#[cfg(all(feature = "serde", feature = "hedging"))]
#[test]
fn hedging_configuration_serialization_roundtrips() {
    let default = seatbelt::hedging::HedgingConfig::default();
    assert_eq!(
        serde_json::to_string(&default).unwrap(),
        r#"{"enabled":true,"hedging_delay":"500ms","max_hedged_attempts":1,"handle_unavailable":false}"#
    );

    let mut config = default;
    config.enabled = false;
    config.hedging_delay = Duration::from_secs(2);
    config.max_hedged_attempts = 4;
    config.handle_unavailable = true;

    let json = serde_json::to_string(&config).unwrap();
    assert_eq!(serde_json::from_str::<seatbelt::hedging::HedgingConfig>(&json).unwrap(), config);
}

#[cfg(all(feature = "serde", feature = "chaos-injection"))]
#[test]
fn injection_configuration_serialization_roundtrips() {
    let default = seatbelt::chaos::injection::InjectionConfig::default();
    assert_eq!(serde_json::to_string(&default).unwrap(), r#"{"enabled":true,"rate":0.0}"#);

    let mut config = default;
    config.enabled = false;
    config.rate = 0.42;

    let json = serde_json::to_string(&config).unwrap();
    assert_eq!(
        serde_json::from_str::<seatbelt::chaos::injection::InjectionConfig>(&json).unwrap(),
        config
    );
}

#[cfg(all(feature = "serde", feature = "chaos-latency"))]
#[test]
fn latency_configuration_serialization_roundtrips() {
    let default = seatbelt::chaos::latency::LatencyConfig::default();
    assert_eq!(
        serde_json::to_string(&default).unwrap(),
        r#"{"enabled":true,"rate":0.0,"latency":"0s","max_latency":null}"#
    );

    let mut config = default;
    config.enabled = false;
    config.rate = 0.42;
    config.latency = Duration::from_millis(100);
    config.max_latency = Some(Duration::from_millis(500));

    let json = serde_json::to_string(&config).unwrap();
    assert_eq!(
        serde_json::from_str::<seatbelt::chaos::latency::LatencyConfig>(&json).unwrap(),
        config
    );
}
