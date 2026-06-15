// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Integration test verifying that cachet's tracing emission paths don't
//! panic when no `tracing` subscriber is installed.
//!
//! This test lives in its own integration-test binary deliberately. The
//! straightforward unit-test version
//! (call `record_get_error` / `record_insert_rejected` etc. with no
//! `set_default` on the current thread) is unsafe to run inside the cachet
//! unit-test binary because of a process-wide tracing-core hazard:
//!
//! * `tracing_core::callsite::DefaultCallsite::register` -- invoked the first
//!   time a `tracing::error!` / `tracing::info!` macro is hit -- calls
//!   `rebuild_callsite_interest(self, &DISPATCHERS.rebuilder())`.
//! * When the registered dispatcher count is `<= 1` (which includes the
//!   "no dispatchers at all" case), `Dispatchers::has_just_one` is `true` and
//!   the rebuilder takes its `JustOne` fast path, which queries
//!   `dispatcher::get_default()` **on the registering thread**.
//! * With no thread-local default and no global default, `get_default`
//!   resolves to `NoSubscriber`, whose `register_callsite` returns
//!   `Interest::never()`. That decision is then cached process-wide, so
//!   every subsequent emission at that callsite -- from ANY thread,
//!   regardless of which subscriber that thread later installs -- is
//!   silently suppressed.
//!
//! That manifests as a flake in
//! `telemetry::cache::tests::every_helper_emits_its_event` (assertion
//! failures on `cache.get_error` / `cache.insert_rejected` with an empty
//! capture buffer).
//!
//! Integration tests each compile to their own binary with their own
//! `tracing-core` callsite registry, so running this scenario here cannot
//! poison any other test binary's callsite cache.
//!
//! The `memory`, `logs`, and `test-util` features required for this test
//! are declared in `Cargo.toml` via `required-features` on the
//! `[[test]] name = "no_subscriber"` entry, so no `#![cfg(...)]` gate is
//! needed here.

use cachet::{Cache, CacheEntry, InsertPolicy, MockCache};
use tick::Clock;

#[cfg_attr(miri, ignore)]
#[tokio::test]
async fn logging_enabled_without_subscriber_does_not_panic() {
    // Deliberately: no `tracing::subscriber::set_default` / `set_global_default`
    // call here, and no other test runs in this binary. The caches below have
    // `enable_logs()` set, so each operation invokes `tracing::error!` /
    // `tracing::info!` / `tracing::debug!` macros inside cachet. Those macros
    // must degrade to a no-op without panicking.

    // Guard the test's core precondition: if some future change inadvertently
    // installs a subscriber (e.g., via a global default in a dependency), the
    // test would silently lose its meaning. Fail loudly instead.
    tracing::dispatcher::get_default(|dispatch| {
        assert!(
            dispatch.is::<tracing::subscriber::NoSubscriber>(),
            "test precondition violated: a tracing subscriber is installed, \
             but this test must run with no subscriber to exercise the \
             no-subscriber code paths; got {dispatch:?}"
        );
    });

    let clock = Clock::new_frozen();

    // (1) Always-failing storage -> covers the error-path tracing events:
    //     `cache.get_error`, `cache.insert_error`, `cache.invalidate_error`,
    //     `cache.clear_error`.
    let failing: MockCache<String, i32> = MockCache::new();
    failing.fail_when(|_| true);
    let failing_cache: Cache<String, i32> = Cache::builder::<String, i32>(clock.clone())
        .name("no_sub_failing")
        .storage(failing)
        .enable_logs()
        .build();
    failing_cache.get(&"k".to_string()).await.unwrap_err();
    failing_cache.insert("k".to_string(), CacheEntry::new(1)).await.unwrap_err();
    failing_cache.invalidate(&"k".to_string()).await.unwrap_err();
    failing_cache.clear().await.unwrap_err();

    // (2) Working storage with `InsertPolicy::never()` -> covers the
    //     success/miss/insert-rejected tracing events: `cache.miss`,
    //     `cache.insert_rejected`, plus `complete_operation`. `insert` returns
    //     `Ok(())` when the policy rejects (rejection is not an error), and
    //     the subsequent `get` must still miss because nothing was stored.
    let rejecting_cache: Cache<String, i32> = Cache::builder::<String, i32>(clock.clone())
        .name("no_sub_rejecting")
        .storage(MockCache::<String, i32>::new())
        .insert_policy(InsertPolicy::never())
        .enable_logs()
        .build();
    assert!(rejecting_cache.get(&"k".to_string()).await.unwrap().is_none());
    rejecting_cache.insert("k".to_string(), CacheEntry::new(1)).await.unwrap();
    assert!(rejecting_cache.get(&"k".to_string()).await.unwrap().is_none());

    // (3) Working storage with default policy -> covers `cache.hit` and
    //     `cache.inserted` (and the success path of `complete_operation`).
    let working_cache: Cache<String, i32> = Cache::builder::<String, i32>(clock)
        .name("no_sub_working")
        .storage(MockCache::<String, i32>::new())
        .enable_logs()
        .build();
    working_cache.insert("k".to_string(), CacheEntry::<i32>::new(1)).await.unwrap();
    assert!(working_cache.get(&"k".to_string()).await.unwrap().is_some());

    // No panic up to this point = every cachet tracing emission path tolerates
    // the absence of a subscriber.
}
