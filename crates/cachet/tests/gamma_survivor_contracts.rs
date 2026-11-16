// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Deterministic cache ordering, timestamps, errors, and telemetry.

testing_aids::init_tracing!();

use std::collections::HashMap;
use std::future::ready;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cachet::{
    Cache, CacheEntry, CacheEventHandler, CacheOperationEvent, CacheTierEvent, Error, InsertPolicy, TransformCodec, TransformEncoder,
    infallible, infallible_owned,
};
use cachet_tier::{CacheOp, CacheTier as _, MockCache};
use futures::executor::block_on;
use tick::Clock;

#[derive(Debug, PartialEq, Eq)]
enum Event {
    Tier(String, String, bool, Duration),
    Operation(String, bool, Duration),
}

#[cfg(test)]
#[derive(Clone, Default)]
struct Recorder(Arc<Mutex<Vec<Event>>>, Arc<Mutex<Option<futures::channel::oneshot::Sender<()>>>>);

#[cfg(test)]
impl CacheEventHandler for Recorder {
    fn on_tier_event(&self, event: &CacheTierEvent<'_>) {
        assert_ne!(event.request_id, 0);
        self.0.lock().unwrap().push(Event::Tier(
            event.tier_name.into(),
            event.outcome.into(),
            event.fallback,
            event.duration,
        ));
    }

    fn on_operation_complete(&self, event: &CacheOperationEvent<'_>) {
        assert_ne!(event.request_id, 0);
        self.0
            .lock()
            .unwrap()
            .push(Event::Operation(event.operation.into(), event.coalesced, event.duration));
        if event.operation == "cache.refresh"
            && let Some(completed) = self.1.lock().unwrap().take()
        {
            completed.send(()).unwrap();
        }
    }
}

fn tier(name: &str, outcome: &str, fallback: bool) -> Event {
    Event::Tier(name.into(), outcome.into(), fallback, Duration::ZERO)
}

fn operation(name: &str, coalesced: bool) -> Event {
    Event::Operation(name.into(), coalesced, Duration::ZERO)
}

#[test]
fn gamma_survivor_fallback_reports_all_success_and_error_outcomes_with_correct_role() {
    block_on(async {
        let clock = Clock::new_frozen();
        let primary = MockCache::<u32, u32>::new();
        let fallback = MockCache::<u32, u32>::new();
        let recorder = Recorder::default();
        let cache = Cache::builder(clock.clone())
            .name("primary")
            .storage(primary.clone())
            .event_handler(recorder.clone())
            .fallback(Cache::builder(clock).name("fallback").storage(fallback.clone()))
            .build();
        cache.insert(7, 13).await.unwrap();
        assert_eq!(
            *recorder.0.lock().unwrap(),
            vec![
                tier("primary", "cache.inserted", false),
                tier("fallback", "cache.inserted", true),
                operation("cache.insert", false),
            ]
        );
        recorder.0.lock().unwrap().clear();
        cache.invalidate(&7).await.unwrap();
        assert_eq!(
            *recorder.0.lock().unwrap(),
            vec![
                tier("primary", "cache.invalidated", false),
                tier("fallback", "cache.invalidated", true),
                operation("cache.invalidate", false),
            ]
        );
        recorder.0.lock().unwrap().clear();
        cache.clear().await.unwrap();
        assert_eq!(
            *recorder.0.lock().unwrap(),
            vec![
                tier("primary", "cache.cleared", false),
                tier("fallback", "cache.cleared", true),
                operation("cache.clear", false),
            ]
        );
        primary.fail_when(|_| true);
        fallback.fail_when(|_| true);
        for (outcome, op) in [
            ("cache.insert_error", "cache.insert"),
            ("cache.invalidate_error", "cache.invalidate"),
            ("cache.clear_error", "cache.clear"),
        ] {
            recorder.0.lock().unwrap().clear();
            let result = match op {
                "cache.insert" => cache.insert(7, 13).await.map(|_| ()),
                "cache.invalidate" => cache.invalidate(&7).await,
                _ => cache.clear().await,
            };
            result.unwrap_err();
            assert_eq!(
                *recorder.0.lock().unwrap(),
                vec![
                    tier("primary", outcome, false),
                    tier("fallback", outcome, true),
                    operation(op, false),
                ]
            );
        }
    });
}

#[test]
fn gamma_survivor_lookup_errors_do_not_run_factories_or_turn_into_panics() {
    block_on(async {
        let storage = MockCache::<u32, u32>::new();
        storage.fail_when(|_| true);
        let cache = Cache::builder(Clock::new_frozen()).storage(storage).build();
        cache.contains(&7).await.unwrap_err();
        cache
            .get_or_insert(&7, || async { panic!("factory must not run after lookup failure") })
            .await
            .unwrap_err();
        cache
            .get_or_insert_with(&7, || async { panic!("factory must not run after lookup failure") })
            .await
            .unwrap_err();
        assert!(
            cache
                .try_get_or_insert_with(&7, || async {
                    Err::<CacheEntry<u32>, _>(Error::from_message("factory must not run"))
                })
                .await
                .unwrap_err()
                .to_string()
                .contains("mock: get failed")
        );
        cache.try_get_or_insert(&7, || ready(Ok::<_, Error>(13))).await.unwrap_err();
        cache.optionally_get_or_insert(&7, || ready(Some(13))).await.unwrap_err();
    });
}

#[test]
fn gamma_survivor_computed_entry_timestamp_reaches_storage_and_insert_errors_propagate() {
    block_on(async {
        let clock = Clock::new_frozen();
        let storage = MockCache::<u32, u32>::new();
        let cache = Cache::builder(clock.clone()).storage(storage.clone()).build();
        let entry = cache.get_or_insert(&7, || ready(13)).await.unwrap();
        assert_eq!(entry.cached_at(), Some(clock.system_time()));
        assert_eq!(storage.get(&7).await.unwrap().unwrap().cached_at(), entry.cached_at());
        storage.fail_when(|op| matches!(op, CacheOp::Insert { .. }));
        assert!(
            cache
                .get_or_insert(&9, || ready(19))
                .await
                .unwrap_err()
                .to_string()
                .contains("mock: insert failed")
        );
    });
}

#[test]
fn gamma_survivor_expired_fallback_and_policy_rejections_emit_exact_callbacks() {
    block_on(async {
        let clock = Clock::new_frozen();
        let recorder = Recorder::default();
        let mut expired = CacheEntry::new(13);
        expired.ensure_cached_at(clock.system_time() - Duration::from_secs(2));
        expired.set_ttl(Duration::from_secs(1));
        let fallback = MockCache::with_data(HashMap::from([(7, expired)]));
        let cache = Cache::builder(clock.clone())
            .name("primary")
            .storage(MockCache::<u32, u32>::new())
            .event_handler(recorder.clone())
            .fallback(Cache::builder(clock.clone()).name("fallback").storage(fallback.clone()))
            .build();
        assert!(cache.get(&7).await.unwrap().is_none());
        assert_eq!(
            *recorder.0.lock().unwrap(),
            vec![
                tier("primary", "cache.miss", false),
                tier("fallback", "cache.expired", true),
                operation("cache.get", false),
            ]
        );
        recorder.0.lock().unwrap().clear();
        fallback.fail_when(|_| true);
        cache.get(&7).await.unwrap_err();
        assert_eq!(
            *recorder.0.lock().unwrap(),
            vec![
                tier("primary", "cache.miss", false),
                tier("fallback", "cache.get_error", true),
                operation("cache.get", false),
            ]
        );
        recorder.0.lock().unwrap().clear();
        let cache = Cache::builder(clock)
            .name("reject")
            .storage(MockCache::<u32, u32>::new())
            .insert_policy(InsertPolicy::never())
            .event_handler(recorder.clone())
            .build();
        assert_eq!(cache.insert(7, 13).await.unwrap(), cachet::InsertOutcome::Rejected);
        assert_eq!(
            *recorder.0.lock().unwrap(),
            vec![tier("reject", "cache.insert_rejected", false), operation("cache.insert", false),]
        );
    });
}

#[test]
fn gamma_survivor_stampede_configuration_survives_fallback_and_transform_builders() {
    block_on(async {
        for enabled in [false, true] {
            let clock = Clock::new_frozen();
            let recorder = Recorder::default();
            let builder = Cache::builder(clock.clone())
                .storage(MockCache::<u32, u32>::new())
                .event_handler(recorder.clone())
                .fallback(Cache::builder(clock).storage(MockCache::<u32, u32>::new()));
            let builder = if enabled { builder.stampede_protection() } else { builder };
            let cache = builder.build();
            cache.get(&7).await.unwrap();
            cache.invalidate(&7).await.unwrap();
            cache.get_or_insert(&7, || ready(13)).await.unwrap();
            cache.try_get_or_insert(&9, || ready(Ok::<_, Error>(19))).await.unwrap();
            cache.optionally_get_or_insert(&11, || ready(Some(23))).await.unwrap();
            let ops: Vec<_> = recorder
                .0
                .lock()
                .unwrap()
                .iter()
                .filter_map(|event| match event {
                    Event::Operation(name, coalesced, _) => Some((name.clone(), *coalesced)),
                    Event::Tier(..) => None,
                })
                .collect();
            assert_eq!(ops.iter().map(|(_, flag)| *flag).collect::<Vec<_>>(), vec![enabled; 5]);

            let clock = Clock::new_frozen();
            let recorder = Recorder::default();
            let builder = Cache::builder(clock.clone())
                .storage(MockCache::<u32, u32>::new())
                .event_handler(recorder.clone());
            let builder = if enabled { builder.stampede_protection() } else { builder };
            let cache = builder
                .transform(
                    TransformEncoder::new(infallible(|k: &u32| u64::from(*k))),
                    TransformCodec::new(
                        infallible(|v: &u32| u64::from(*v)),
                        infallible_owned(|v: u64| u32::try_from(v).unwrap()),
                    ),
                )
                .fallback(Cache::builder(clock).storage(MockCache::<u64, u64>::new()))
                .build();
            cache.get(&7).await.unwrap();
            assert_eq!(recorder.0.lock().unwrap().last(), Some(&operation("cache.get", enabled)));
        }
    });
}

#[test]
fn gamma_survivor_transform_debug_labels_and_underlying_lookup_errors_are_preserved() {
    block_on(async {
        let clock = Clock::new_frozen();
        let codec = TransformCodec::new(
            infallible(|v: &u32| u64::from(*v)),
            infallible_owned(|v: u64| u32::try_from(v).unwrap()),
        );
        let encoder = TransformEncoder::new(infallible(|k: &u32| u64::from(*k)));
        for output in [format!("{codec:?}"), format!("{encoder:?}")] {
            assert!(output.contains("A:"), "{output}");
            assert!(output.contains("B:"), "{output}");
        }

        let fallback = MockCache::<u64, u64>::new();
        fallback.fail_when(|_| true);
        let builder = Cache::builder(clock.clone())
            .storage(MockCache::<u32, u32>::new())
            .transform(encoder, codec)
            .fallback(Cache::builder(clock).storage(fallback));
        let output = format!("{builder:?}");
        for field in ["pre:", "post:", "K:", "KT:", "V:", "VT:"] {
            assert!(output.contains(field), "{output}");
        }
        assert!(builder.build().get(&7).await.unwrap_err().to_string().contains("mock: get failed"));
    });
}

#[test]
fn gamma_survivor_refresh_at_exact_boundary_coalesces_and_reports_hit_miss_error() {
    block_on(async {
        for outcome in ["cache.refresh_hit", "cache.refresh_miss", "cache.refresh_error"] {
            let control = tick::ClockControl::new();
            let clock = control.to_clock();
            let mut old = CacheEntry::new(13);
            old.ensure_cached_at(clock.system_time());
            let primary = MockCache::with_data(HashMap::from([(7_u32, old)]));
            let fallback = MockCache::<u32, u32>::new();
            if outcome == "cache.refresh_hit" {
                fallback.insert(7, CacheEntry::new(17)).await.unwrap();
            } else if outcome == "cache.refresh_error" {
                fallback.fail_when(|_| true);
            }
            let recorder = Recorder::default();
            let runtime = tokio::runtime::Builder::new_current_thread().build().unwrap();
            let (completed, completion) = futures::channel::oneshot::channel();
            *recorder.1.lock().unwrap() = Some(completed);
            let cache = Cache::builder(clock.clone())
                .name("primary")
                .storage(primary.clone())
                .event_handler(recorder.clone())
                .fallback(Cache::builder(clock).name("fallback").storage(fallback))
                .time_to_refresh(cachet::TimeToRefresh::new(
                    Duration::from_secs(1),
                    anyspawn::Spawner::new_tokio(runtime.handle().clone()),
                ))
                .build();
            assert_eq!(*cache.get(&7).await.unwrap().unwrap(), 13);
            assert_eq!(runtime.metrics().num_alive_tasks(), 0);
            control.advance(Duration::from_secs(1));
            assert_eq!(*cache.get(&7).await.unwrap().unwrap(), 13);
            assert_eq!(runtime.metrics().num_alive_tasks(), 1);
            assert_eq!(*cache.get(&7).await.unwrap().unwrap(), 13);
            assert_eq!(runtime.metrics().num_alive_tasks(), 1);
            runtime.block_on(completion).unwrap();
            assert_eq!(
                *primary.get(&7).await.unwrap().unwrap(),
                if outcome == "cache.refresh_hit" { 17 } else { 13 }
            );
            let refresh: Vec<_> = recorder
                .0
                .lock()
                .unwrap()
                .iter()
                .filter(|event| match event {
                    Event::Tier(_, event, _, _) => event.starts_with("cache.refresh_"),
                    Event::Operation(event, _, _) => event == "cache.refresh",
                })
                .map(|event| match event {
                    Event::Tier(name, outcome, fallback, duration) => Event::Tier(name.clone(), outcome.clone(), *fallback, *duration),
                    Event::Operation(name, coalesced, duration) => Event::Operation(name.clone(), *coalesced, *duration),
                })
                .collect();
            assert_eq!(refresh, [tier("primary", outcome, true), operation("cache.refresh", false)]);
        }
    });
}

#[cfg(feature = "logs")]
#[test]
fn gamma_survivor_disabled_logs_do_not_emit_any_tier_or_operation_events() {
    let capture = testing_aids::tracing_logs::Capture::new();
    let _guard = tracing::subscriber::set_default(capture.subscriber());
    block_on(async {
        let cache = Cache::builder(Clock::new_frozen()).storage(MockCache::<u32, u32>::new()).build();
        cache.get(&7).await.unwrap();
        cache.insert(7, 13).await.unwrap();
        cache.invalidate(&7).await.unwrap();
        cache.clear().await.unwrap();
        let cache = Cache::builder(Clock::new_frozen())
            .storage(MockCache::<u32, u32>::new())
            .insert_policy(InsertPolicy::never())
            .build();
        cache.insert(7, 13).await.unwrap();
    });
    assert_eq!(capture.output(), "");
    assert_eq!(
        format!("{:?}", cachet::telemetry::CacheTelemetry::default()),
        "CacheTelemetry { logging_enabled: false, has_handler: false }"
    );
}

#[cfg(feature = "service")]
#[test]
fn gamma_survivor_service_operations_forward_storage_errors_without_panicking() {
    use cachet_service::{CacheOperation, GetRequest, InsertRequest, InvalidateRequest};
    use layered::Service as _;

    block_on(async {
        let storage = MockCache::<u32, u32>::new();
        storage.fail_when(|_| true);
        let cache = Cache::builder(Clock::new_frozen()).storage(storage).build();
        for request in [
            CacheOperation::Get(GetRequest::new(7)),
            CacheOperation::Insert(InsertRequest::new(7, CacheEntry::new(13))),
            CacheOperation::Invalidate(InvalidateRequest::new(7)),
            CacheOperation::Clear,
        ] {
            cache.execute(request).await.unwrap_err();
        }
    });
}
