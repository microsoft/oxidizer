// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Configuration through public APIs.

#![cfg(test)]

mod builder {
    use std::time::Duration;

    use cachet_memory::InMemoryCacheBuilder;

    #[test]
    fn eviction_telemetry_defaults_false() {
        let builder = InMemoryCacheBuilder::<String, i32>::new();
        assert!(!builder.eviction_telemetry_enabled());
    }

    #[test]
    fn with_eviction_telemetry_sets_flag() {
        let builder = InMemoryCacheBuilder::<String, i32>::new().with_eviction_telemetry();
        assert!(builder.eviction_telemetry_enabled());
    }

    #[test]
    fn with_hasher_preserves_eviction_telemetry_flag() {
        let builder = InMemoryCacheBuilder::<String, i32>::new()
            .with_eviction_telemetry()
            .with_hasher(std::collections::hash_map::RandomState::new());
        assert!(builder.eviction_telemetry_enabled());
    }

    #[test]
    fn debug_impl_renders_all_fields() {
        let builder = InMemoryCacheBuilder::<String, i32>::new()
            .max_capacity(100)
            .initial_capacity(10)
            .time_to_live(Duration::from_mins(1))
            .time_to_idle(Duration::from_secs(30))
            .name("my_cache")
            .with_eviction_telemetry()
            .with_removal_observer(|_| {})
            .on_eviction(|_, _, _| {});
        let rendered = format!("{builder:?}");
        assert!(rendered.contains("InMemoryCacheBuilder"));
        assert!(rendered.contains("max_capacity: Some(100)"));
        assert!(rendered.contains("initial_capacity: Some(10)"));
        assert!(rendered.contains("time_to_live: Some(60s)"));
        assert!(rendered.contains("time_to_idle: Some(30s)"));
        assert!(rendered.contains("name: Some(\"my_cache\")"));
        assert!(rendered.contains("eviction_telemetry: true"));
        assert!(rendered.contains("removal_observers: 1"));
        assert!(rendered.contains("eviction_listener: Some(\"<set>\")"));
    }

    #[test]
    fn build_max_capacity_lt_initial_capacity_returns_validation_error() {
        let result = InMemoryCacheBuilder::<String, i32>::new()
            .max_capacity(100)
            .initial_capacity(101)
            .build();
        ohno::assert_error_message!(
            result.unwrap_err(),
            "invalid cache configuration: initial_capacity (101) exceeds max_capacity (100)"
        );
    }

    #[cfg_attr(miri, ignore)] // crossbeam-epoch triggers Stacked Borrows violations under Miri
    #[test]
    fn build_max_capacity_eq_initial_capacity_succeeds() {
        let result = InMemoryCacheBuilder::<String, i32>::new()
            .max_capacity(100)
            .initial_capacity(100)
            .build();
        result.unwrap();
    }

    #[test]
    fn build_ttl_less_than_tti_returns_validation_error() {
        let result = InMemoryCacheBuilder::<String, i32>::new()
            .time_to_live(Duration::from_mins(1))
            .time_to_idle(Duration::from_mins(2))
            .build();
        ohno::assert_error_message!(
            result.unwrap_err(),
            "invalid cache configuration: time to idle (120s) exceeds time to live (60s)."
        );
    }

    #[cfg_attr(miri, ignore)] // crossbeam-epoch triggers Stacked Borrows violations under Miri
    #[test]
    fn build_ttl_eq_tti_succeeds() {
        let result = InMemoryCacheBuilder::<String, i32>::new()
            .time_to_live(Duration::from_mins(1))
            .time_to_idle(Duration::from_mins(1))
            .build();
        result.unwrap();
    }
}
