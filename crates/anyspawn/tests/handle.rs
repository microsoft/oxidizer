// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![allow(missing_docs, reason = "test code")]
#![expect(clippy::unwrap_used, reason = "test failures use unwrap for concise backtraces")]

//! Tests for `JoinHandle` implementations.

use std::error::Error as _;
use std::future;

use anyspawn::{JoinHandle, Spawner, TokioDeps};

static_assertions::assert_impl_all!(anyspawn::JoinHandle<()>: Send, Unpin);
static_assertions::assert_not_impl_any!(anyspawn::JoinHandle<()>: Sync);

#[cfg_attr(miri, ignore)]
#[tokio::test]
async fn join_handle_debug() {
    let spawner = Spawner::new_tokio(TokioDeps::new());
    let handle = spawner.spawn(async { 42 });
    let debug_str = format!("{handle:?}");
    assert!(debug_str.contains("JoinHandle"));
    assert_eq!(handle.await.unwrap(), 42);
}

#[test]
fn tokio_runtime_shutdown_reports_the_join_error() -> std::io::Result<()> {
    let runtime = tokio::runtime::Builder::new_current_thread().build()?;
    let spawner = Spawner::new_tokio(TokioDeps::with_handle(runtime.handle().clone()));
    let handle = spawner.spawn(future::pending::<()>());
    drop(runtime);

    let error = futures::executor::block_on(handle).unwrap_err();

    assert!(error.is_cancelled());
    assert!(!error.is_panic());
    assert!(
        error
            .source()
            .unwrap()
            .downcast_ref::<tokio::task::JoinError>()
            .unwrap()
            .is_cancelled()
    );
    assert!(error.to_string().contains("cancelled"));
    Ok(())
}

#[tokio::test]
async fn tokio_task_panics_return_join_errors() {
    let spawner = Spawner::new_tokio(TokioDeps::default());
    let handles: [JoinHandle<()>; 3] = [
        spawner.spawn(async { panic!("async task failed") }),
        spawner.spawn_anywhere((), |()| async { panic!("anywhere task failed") }),
        spawner.spawn_blocking(|| panic!("blocking task failed")),
    ];

    for handle in handles {
        let error = handle.await.unwrap_err();
        assert!(error.is_panic());
        assert!(!error.is_cancelled());
        assert!(error.source().unwrap().downcast_ref::<tokio::task::JoinError>().unwrap().is_panic());
        assert!(error.to_string().contains("task failed"));
    }
}

#[tokio::test]
async fn task_error_is_separate_from_join_error() {
    let spawner = Spawner::new_tokio(TokioDeps::default());
    let result = spawner.spawn(async { Err::<(), _>("task failed") }).await.unwrap();
    assert_eq!(result, Err("task failed"));
}
