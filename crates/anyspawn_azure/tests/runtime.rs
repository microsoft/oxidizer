// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Integration tests for [`fetch_azure::Runtime`].
//!
//! These drive the runtime adapter on a real Tokio spawner and clock.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyspawn::Spawner;
use anyspawn_azure::Runtime;
use azure_core::async_runtime::AsyncRuntime;
use azure_core::time::Duration;
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
    // await resolves rather than hanging forever. The timeout bounds the wait so
    // a broken `abort` fails the test promptly instead of hanging.
    let task = runtime.spawn(Box::pin(std::future::pending::<()>()));
    task.abort();

    tokio::time::timeout(std::time::Duration::from_secs(10), task)
        .await
        .expect("abort should wake the waiter so the task resolves promptly")
        .unwrap();
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
async fn runtime_accessors_round_trip() {
    let runtime = Runtime::new(Spawner::new_tokio(), Clock::new_tokio());

    // `spawner` and `clock` expose the wrapped components; rebuild from them.
    let runtime = Runtime::new(runtime.spawner().clone(), runtime.clock().clone());

    runtime.yield_now().await;
}

#[cfg(feature = "azure-identity")]
#[tokio::test]
async fn runtime_executor_runs_command() {
    use std::ffi::OsStr;

    use azure_identity::Executor;

    let runtime = Runtime::new(Spawner::new_tokio(), Clock::new_tokio());

    #[cfg(windows)]
    let output = runtime
        .run(OsStr::new("cmd"), &[OsStr::new("/C"), OsStr::new("echo hello")])
        .await
        .unwrap();
    #[cfg(not(windows))]
    let output = runtime
        .run(OsStr::new("/bin/sh"), &[OsStr::new("-c"), OsStr::new("echo hello")])
        .await
        .unwrap();

    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("hello"));
}
