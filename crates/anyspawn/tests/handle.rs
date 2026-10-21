// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![allow(missing_docs, reason = "test code")]

//! Tests for `JoinHandle` implementations.

use std::future;
use std::panic::{AssertUnwindSafe, catch_unwind};

use anyspawn::{BoxedBlockingTask, BoxedFuture, SpawnCustom, Spawner};
use thread_aware::ThreadAware;
use thread_aware::closure::ThreadAwareAsyncFnOnce;

static_assertions::assert_impl_all!(anyspawn::JoinHandle<()>: Send, Unpin);
static_assertions::assert_not_impl_any!(anyspawn::JoinHandle<()>: Sync);

#[derive(Clone, ThreadAware)]
struct DroppingSpawner;

impl SpawnCustom for DroppingSpawner {
    fn spawn(&self, _task: BoxedFuture) {}

    fn spawn_anywhere(&self, _task: Box<dyn ThreadAwareAsyncFnOnce<()>>) {}

    fn spawn_blocking(&self, _task: BoxedBlockingTask) {}
}

fn panic_message(panic: &(dyn std::any::Any + Send)) -> &str {
    panic
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| panic.downcast_ref::<&str>().copied())
        .unwrap()
}

#[cfg_attr(miri, ignore)]
#[tokio::test]
async fn join_handle_debug() {
    let spawner = Spawner::new_tokio();
    let handle = spawner.spawn(async { 42 });
    let debug_str = format!("{handle:?}");
    assert!(debug_str.contains("JoinHandle"));
    let _ = handle.await;
}

#[test]
fn dropped_custom_task_reports_the_channel_error() {
    let spawner = Spawner::new_custom("dropping", DroppingSpawner);
    let handle = spawner.spawn(async {});

    let panic = catch_unwind(AssertUnwindSafe(|| futures::executor::block_on(handle))).unwrap_err();

    assert!(panic_message(&*panic).contains("channel closed"));
}

#[test]
fn tokio_runtime_shutdown_reports_the_join_error() -> std::io::Result<()> {
    let runtime = tokio::runtime::Builder::new_current_thread().build()?;
    let spawner = Spawner::new_tokio_with_handle(runtime.handle().clone());
    let handle = spawner.spawn(future::pending::<()>());
    drop(runtime);

    let panic = catch_unwind(AssertUnwindSafe(|| futures::executor::block_on(handle))).unwrap_err();

    assert!(panic_message(&*panic).contains("was cancelled"));
    Ok(())
}
