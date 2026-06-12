// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Integration tests for [`fetch_azure::Runtime`].
//!
//! These drive the runtime adapter on a real Tokio spawner and clock.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyspawn::Spawner;
use azure_core::async_runtime::AsyncRuntime;
use azure_core::time::Duration;
use fetch_azure::Runtime;
use tick::Clock;

#[tokio::test]
async fn runtime_spawn_runs_task_to_completion() {
    let runtime = Runtime::new(Spawner::new_tokio(), Clock::new_tokio());
    let ran = Arc::new(AtomicBool::new(false));
    let ran_in_task = Arc::clone(&ran);

    let task = runtime.spawn(Box::pin(async move {
        ran_in_task.store(true, Ordering::SeqCst);
    }));
    task.await.unwrap();

    assert!(ran.load(Ordering::SeqCst));
}

#[tokio::test]
async fn runtime_abort_resolves_without_waiting() {
    let runtime = Runtime::new(Spawner::new_tokio(), Clock::new_tokio());

    // The task never completes on its own; aborting must wake the waiter so the
    // await resolves rather than hanging forever.
    let task = runtime.spawn(Box::pin(std::future::pending::<()>()));
    task.abort();
    task.await.unwrap();
}

#[tokio::test]
async fn runtime_sleep_completes() {
    let runtime = Runtime::new(Spawner::new_tokio(), Clock::new_tokio());

    runtime.sleep(Duration::milliseconds(1)).await;
}

#[tokio::test]
async fn runtime_yield_now_completes() {
    let runtime = Runtime::new(Spawner::new_tokio(), Clock::new_tokio());

    runtime.yield_now().await;
}

#[tokio::test]
async fn runtime_converts_into_dyn_runtime() {
    let runtime: Arc<dyn AsyncRuntime> = Runtime::new(Spawner::new_tokio(), Clock::new_tokio()).into();

    runtime.spawn(Box::pin(async {})).await.unwrap();
}

#[tokio::test]
async fn runtime_from_spawner_clock_and_accessors_round_trip() {
    let runtime = Runtime::from((Spawner::new_tokio(), Clock::new_tokio()));

    // `spawner` and `clock` expose the wrapped components; rebuild from them.
    let runtime = Runtime::new(runtime.spawner().clone(), runtime.clock().clone());

    runtime.yield_now().await;
}
