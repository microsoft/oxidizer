// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![allow(missing_docs, reason = "This is a test module")]

use std::fmt::{Debug, Display};

#[cfg(feature = "breaker")]
use std::borrow::Cow;

use seatbelt::{Attempt, ResilienceContext};
use static_assertions::assert_impl_all;
use thread_aware::ThreadAware;

assert_impl_all!(Attempt: Send, Sync, Unpin, Copy, Clone, Debug, Display, PartialEq, Eq);
assert_impl_all!(ResilienceContext<(), ()>: Send, Sync, ThreadAware, Debug, Clone);

#[test]
fn attempt_reports_public_state() {
    let first = Attempt::new(0, false);
    assert_eq!(first.index(), 0);
    assert!(first.is_first());
    assert!(!first.is_last());

    let last = Attempt::new(5, true);
    assert_eq!(last.index(), 5);
    assert!(!last.is_first());
    assert!(last.is_last());
}

#[test]
fn attempt_default_is_a_single_execution() {
    let attempt = Attempt::default();
    assert_eq!(attempt.index(), 0);
    assert!(attempt.is_first());
    assert!(attempt.is_last());
}

#[test]
fn attempt_display_contains_the_index() {
    assert_eq!(Attempt::new(42, false).to_string(), "42");
}

#[cfg(feature = "breaker")]
mod breaker {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    use std::time::Duration;

    use seatbelt::breaker::{AbandonedPolicy, BreakerId, HalfOpenMode};
    use static_assertions::assert_impl_all;

    use super::*;

    assert_impl_all!(BreakerId: Send, Sync, Unpin, Clone, Hash, Display, Debug, PartialEq, Eq);

    #[test]
    fn breaker_id_conversions_preserve_the_display_value() {
        let number = BreakerId::from(42_u64);
        let borrowed = BreakerId::from("hello");
        let owned = BreakerId::from(String::from("hello"));

        assert_eq!(number.to_string(), "42");
        assert_eq!(borrowed.to_string(), "hello");
        assert_eq!(owned.to_string(), "hello");
        assert_eq!(borrowed, owned);
    }

    #[test]
    fn breaker_id_hash_is_stable_for_equal_values() {
        let first = BreakerId::from(123_u64);
        let second = BreakerId::from(123_u64);

        let mut first_hasher = DefaultHasher::new();
        first.hash(&mut first_hasher);
        let mut second_hasher = DefaultHasher::new();
        second.hash(&mut second_hasher);

        assert_eq!(first_hasher.finish(), second_hasher.finish());
    }

    #[test]
    fn hashed_breaker_ids_use_the_public_label() {
        let first = BreakerId::hashed(&"value", "label");
        let second = BreakerId::hashed(&"value", "label");
        let different = BreakerId::hashed(&"other", "label");

        assert_eq!(first.to_string(), "label");
        assert_eq!(first, second);
        assert_ne!(first, different);
    }

    #[test]
    fn breaker_id_converts_to_the_expected_cow_kind() {
        let number: Cow<'static, str> = BreakerId::from(42_u64).into();
        let owned: Cow<'static, str> = BreakerId::from(String::from("owned")).into();
        let borrowed: Cow<'static, str> = BreakerId::from("borrowed").into();
        let hashed: Cow<'static, str> = BreakerId::hashed(&"value", "label").into();

        assert!(matches!(number, Cow::Owned(_)));
        assert_eq!(number, "42");
        assert!(matches!(owned, Cow::Owned(_)));
        assert_eq!(owned, "owned");
        assert!(matches!(borrowed, Cow::Borrowed(_)));
        assert_eq!(borrowed, "borrowed");
        assert!(matches!(hashed, Cow::Borrowed(_)));
        assert_eq!(hashed, "label");
    }

    #[test]
    fn abandoned_policy_default_matches_the_default_threshold() {
        assert_eq!(AbandonedPolicy::default(), AbandonedPolicy::rate_threshold(1.0));
    }

    #[test]
    #[should_panic(expected = "threshold must be in (0.0, 1.0]")]
    fn abandoned_policy_rejects_a_threshold_above_one() {
        let _ = AbandonedPolicy::rate_threshold(1.5);
    }

    #[test]
    #[should_panic(expected = "threshold must be in (0.0, 1.0]")]
    fn abandoned_policy_rejects_a_zero_threshold() {
        let _ = AbandonedPolicy::rate_threshold(0.0);
    }

    #[test]
    fn half_open_mode_default_is_progressive_without_a_duration() {
        assert_eq!(HalfOpenMode::default(), HalfOpenMode::progressive(None));
    }

    #[test]
    fn half_open_mode_accepts_optional_and_direct_durations() {
        let duration = Duration::from_secs(10);
        assert_eq!(HalfOpenMode::progressive(duration), HalfOpenMode::progressive(Some(duration)));
        assert_ne!(HalfOpenMode::quick(), HalfOpenMode::progressive(None));
    }

    #[cfg(feature = "serde")]
    mod serde {
        use super::*;

        #[test]
        fn abandoned_policy_preserves_valid_serialized_values() {
            let policy: AbandonedPolicy = serde_json::from_str(r#"{"rate_threshold":0.5}"#).unwrap();
            assert_eq!(policy, AbandonedPolicy::rate_threshold(0.5));
            assert_eq!(serde_json::to_string(&policy).unwrap(), r#"{"rate_threshold":0.5}"#);
        }

        #[test]
        fn abandoned_policy_clamps_serialized_thresholds() {
            let above_one: AbandonedPolicy = serde_json::from_str(r#"{"rate_threshold":1.5}"#).unwrap();
            let zero: AbandonedPolicy = serde_json::from_str(r#"{"rate_threshold":0.0}"#).unwrap();
            let negative: AbandonedPolicy = serde_json::from_str(r#"{"rate_threshold":-0.5}"#).unwrap();

            assert_eq!(above_one, AbandonedPolicy::rate_threshold(1.0));
            assert_eq!(zero, AbandonedPolicy::rate_threshold(f32::MIN_POSITIVE));
            assert_eq!(negative, AbandonedPolicy::rate_threshold(f32::MIN_POSITIVE));
        }

        #[test]
        fn half_open_mode_serialization_roundtrips() {
            let cases = [
                (HalfOpenMode::quick(), r#""quick""#),
                (HalfOpenMode::progressive(None), r#"{"progressive":null}"#),
                (HalfOpenMode::progressive(Duration::from_secs(605)), r#"{"progressive":"10m 5s"}"#),
            ];

            for (mode, expected) in cases {
                let json = serde_json::to_string(&mode).unwrap();
                assert_eq!(json, expected);
                assert_eq!(serde_json::from_str::<HalfOpenMode>(&json).unwrap(), mode);
            }
        }

        #[test]
        fn half_open_mode_clamps_short_stage_durations() {
            let mode = HalfOpenMode::progressive(Duration::from_millis(500));
            assert_eq!(serde_json::to_string(&mode).unwrap(), r#"{"progressive":"1s"}"#);
        }

        #[test]
        fn half_open_mode_deserializes_verbose_durations() {
            let mode: HalfOpenMode = serde_json::from_str(r#"{"progressive":"1 hour, 30 minutes"}"#).unwrap();
            assert_eq!(mode, HalfOpenMode::progressive(Duration::from_mins(90)));
        }

        #[test]
        fn half_open_mode_rejects_invalid_serialized_values() {
            let variant = serde_json::from_str::<HalfOpenMode>(r#""Unknown""#).unwrap_err();
            assert!(variant.to_string().contains("unknown variant"));

            let duration = serde_json::from_str::<HalfOpenMode>(r#"{"progressive":"not_a_duration"}"#).unwrap_err();
            assert!(duration.to_string().contains("friendly"));
        }
    }
}
